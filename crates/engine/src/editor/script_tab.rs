//! Script tabs in the LOGIC workspace: a small code editor for the
//! project's Rhai scripts. Colors by token, line numbers, Tab indents, a
//! live syntax check with the line it's on, and formatting on save (or
//! FORMAT). On the right: where the script is used and what it can call.
//! Text edits are undone inside the editor (Ctrl+Z while typing).

use super::widgets::{Side, collapsed_panel, panel_header};
use super::{Editor, EditorOutput, dim, section};
use crate::script::format::{self, Kind};
use crate::ui::theme;
use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontId, RichText, Ui};
use std::path::{Path, PathBuf};

/// A script open in a tab.
pub struct ScriptDoc {
    pub text: String,
    /// As last saved (or loaded).
    saved: String,
    /// The syntax check and the text it was made for.
    checked: Option<(String, Option<(usize, String)>)>,
}

impl ScriptDoc {
    pub fn new(text: String) -> Self {
        let text = text.replace("\r\n", "\n");
        Self {
            saved: text.clone(),
            text,
            checked: None,
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.text != self.saved
    }

    pub fn mark_saved(&mut self) {
        self.saved = self.text.clone();
    }

    /// The first syntax error and its line, checked again when the text
    /// changed.
    fn error(&mut self) -> Option<(usize, String)> {
        if self.checked.as_ref().is_none_or(|(t, _)| *t != self.text) {
            let error = crate::script::syntax_error(&self.text);
            self.checked = Some((self.text.clone(), error));
        }
        self.checked.as_ref().and_then(|(_, e)| e.clone())
    }

    /// Formats the text, unless it has a syntax error (then it's kept as
    /// typed). Returns whether anything changed.
    fn format(&mut self) -> bool {
        if self.error().is_some() {
            return false;
        }
        let formatted = format::format(&self.text);
        let changed = formatted != self.text;
        self.text = formatted;
        changed
    }
}

/// What a new script starts as: both kinds of entry points, commented.
pub fn template(name: &str) -> String {
    format!(
        "// {name}\n\
         //\n\
         // As an ability: a graph's ABILITY node turns it on, then the engine\n\
         // calls start() once and update(player, dt) every frame.\n\
         // On an object: give it the Script behavior, then start(object) and\n\
         // update(object, dt) run while the game plays.\n\
         // `this` is the script's own memory; dt is seconds since last frame.\n\
         \n\
         fn start() {{\n    this.time = 0.0;\n}}\n\
         \n\
         fn update(player, dt) {{\n    this.time += dt;\n}}\n"
    )
}

/// Colors for the editor, within the amber look.
fn color(kind: Kind) -> Color32 {
    match kind {
        Kind::Keyword => theme::AMBER,
        Kind::Ident => Color32::from_rgb(236, 214, 182),
        Kind::Number => Color32::from_rgb(255, 196, 120),
        Kind::Text => Color32::from_rgb(214, 190, 110),
        Kind::Comment => theme::AMBER_DIM,
        Kind::Punct => Color32::from_rgb(200, 160, 110),
    }
}

/// The text as colored pieces, whitespace left in its own color.
fn highlight(text: &str, font: FontId) -> LayoutJob {
    let mut job = LayoutJob::default();
    let format = |color: Color32| TextFormat {
        font_id: font.clone(),
        color,
        ..Default::default()
    };
    let mut at = 0;
    for token in format::tokens(text) {
        if token.start > at {
            job.append(&text[at..token.start], 0.0, format(theme::AMBER_DIM));
        }
        job.append(
            &text[token.start..token.end],
            0.0,
            format(color(token.kind)),
        );
        at = token.end;
    }
    if at < text.len() {
        job.append(&text[at..], 0.0, format(theme::AMBER_DIM));
    }
    job
}

/// What scripts can call, for the reference card: (what, meaning).
const REFERENCE: &[(&str, &str)] = &[
    ("fn start() / start(object)", "once, when it begins"),
    ("fn update(player, dt)", "every frame, as an ability"),
    ("fn update(object, dt)", "every frame, on an object"),
    ("this.name = value", "the script's own memory"),
    (
        "player.grounded .jumped",
        "on the ground; jumped this frame",
    ),
    ("player.crouching .sprinting", "what the player is doing"),
    ("player.position", "feet position (vec3, read only)"),
    ("player.velocity", "vec3; set it, or .y = 6"),
    ("player.jump()", "a full jump from where it is"),
    ("player.jump_speed", "a normal jump's upward speed"),
    ("player.pressed(\"e\")", "key went down this frame"),
    ("player.held(\"shift\")", "key is down"),
    ("object.name", "the object's name"),
    (
        "object.position .rotation .scale",
        "vec3 (rotation in degrees)",
    ),
    ("object.visible", "shown, with collision"),
    ("player()", "the player, from any script"),
    ("flag(\"name\")", "a flag's value"),
    ("set_flag(\"name\", value)", "set a flag (graphs see it)"),
    ("vec3(x, y, z)", ".x .y .z, + - * /, .length()"),
    ("print(...)", "writes to the log"),
];

impl Editor {
    /// Opens `doc` (read from `path`) in a new tab and makes it active.
    pub fn open_script_tab(&mut self, path: PathBuf, doc: ScriptDoc) {
        self.open_logic(path, super::logic_docs::LogicDoc::Script(doc));
    }

    /// Formats the open script (unless it has an error) and writes it.
    pub(super) fn save_script(&mut self) {
        let Some(doc) = &mut self.script else {
            return;
        };
        let formatted = doc.format();
        let path = self.project_dir.join(&self.logic_path);
        match std::fs::write(&path, &doc.text) {
            Ok(()) => {
                doc.mark_saved();
                log::info!("saved {}", path.display());
                let note = match doc.error() {
                    Some((line, _)) => format!(" (NOT FORMATTED: ERROR ON LINE {line})"),
                    None if formatted => " (FORMATTED)".into(),
                    None => String::new(),
                };
                self.message = Some((format!("SAVED {}{note}", self.logic_path.display()), false));
                self.index.refresh(&self.project_dir, true);
            }
            Err(err) => {
                log::error!("could not write {}: {err}", path.display());
                self.message = Some((format!("COULD NOT SAVE: {err}"), true));
            }
        }
    }

    /// The LOGIC workspace with a script open.
    pub(super) fn script_ui(&mut self, ui: &mut Ui, output: &mut EditorOutput) {
        self.index.refresh(&self.project_dir, false);
        let path = crate::graph::key(&self.logic_path);
        self.logic_side_panel(ui, &path, output);
        if self.show_inspector {
            egui::Panel::right("script_inspector")
                .resizable(true)
                .default_size(340.0)
                .min_size(300.0)
                .show(ui, |ui| {
                    self.show_inspector =
                        !panel_header(ui, "INSPECTOR", Side::Right, "SHIFT+TAB", |_| {});
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| self.script_card(ui, &path, output));
                });
        } else {
            self.show_inspector = collapsed_panel(
                ui,
                "script_inspector_strip",
                Side::Right,
                "INSPECTOR",
                "SHIFT+TAB",
            );
        }
        egui::Panel::top("editor_tabs").show(ui, |ui| self.tab_strip(ui, output));
        let Some(doc) = &mut self.script else {
            return;
        };
        let error = doc.error();
        egui::Panel::bottom("script_status").show(ui, |ui| {
            ui.horizontal(|ui| match &error {
                Some((line, message)) => {
                    ui.label(RichText::new(format!("LINE {line}:")).color(theme::ALERT));
                    ui.label(RichText::new(message).color(theme::ALERT));
                }
                None => {
                    let lines = doc.text.lines().count();
                    ui.label(dim(&format!(
                        "{lines} LINES · NO SYNTAX ERRORS · CTRL+S FORMATS AND SAVES"
                    )));
                }
            });
        });
        let mut format_now = false;
        let mut save_now = false;
        egui::Panel::top("script_toolbar").show(ui, |ui| {
            ui.horizontal(|ui| {
                let can = error.is_none();
                if ui
                    .add_enabled(can, egui::Button::new("FORMAT").small())
                    .on_hover_text("INDENTS AND SPACES, AS ON SAVE")
                    .on_disabled_hover_text("FIX THE SYNTAX ERROR FIRST")
                    .clicked()
                {
                    format_now = true;
                }
                if ui.small_button("SAVE").on_hover_text("CTRL+S").clicked() {
                    save_now = true;
                }
            });
        });
        if format_now {
            doc.format();
        }
        // While typing, egui has the keyboard and the editor's own Ctrl+S
        // doesn't see it.
        let typing = ui.ctx().memory(|m| m.focused().is_some());
        if typing && ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::S)) {
            save_now = true;
        }
        code_editor(ui, doc, error.map(|(line, _)| line));
        if save_now {
            self.save_script();
        }
    }

    /// The script's card: its file, who uses it, and the reference.
    fn script_card(&mut self, ui: &mut Ui, path: &str, output: &mut EditorOutput) {
        section(ui, "SCRIPT");
        egui::Grid::new("script_card")
            .num_columns(2)
            .spacing([12.0, 4.0])
            .show(ui, |ui| {
                ui.label(dim("FILE"));
                ui.label(path);
                ui.end_row();
            });
        let uses = self.index.script_users(path);
        section(ui, &format!("USED BY  {}", uses.len()));
        if uses.is_empty() {
            ui.label(dim("NOTHING YET. AS AN ABILITY: AN ABILITY NODE"));
            ui.label(dim("IN A GRAPH. ON AN OBJECT: ITS SCRIPT BEHAVIOR"));
            ui.label(dim("(BUILD › ADD BEHAVIOR › SCRIPT)."));
        }
        for (text, go) in uses {
            ui.horizontal(|ui| {
                ui.add(egui::Label::new(&text).truncate());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("OPEN").clicked() {
                        match go {
                            super::project_index::Go::Scene(scene) => {
                                let scene = PathBuf::from(scene);
                                match self.tab_of(&scene) {
                                    Some(index) => output.switch_tab = Some(index),
                                    None => output.new_tab = Some(scene),
                                }
                            }
                            super::project_index::Go::Graph(graph) => {
                                output.open_graph = Some(PathBuf::from(graph))
                            }
                        }
                    }
                });
            });
        }
        section(ui, "WHAT A SCRIPT CAN USE");
        egui::Grid::new("script_reference")
            .num_columns(2)
            .spacing([10.0, 3.0])
            .show(ui, |ui| {
                for (what, meaning) in REFERENCE {
                    ui.label(RichText::new(*what).color(theme::AMBER));
                    ui.label(dim(meaning));
                    ui.end_row();
                }
            });
        ui.add_space(6.0);
        ui.label(dim("KEYS: LETTERS, DIGITS, SPACE, SHIFT, CTRL, ALT,"));
        ui.label(dim("TAB, ENTER, UP, DOWN, LEFT, RIGHT."));
    }
}

/// The code: line numbers in a gutter, the text colored by token, the
/// line with an error marked.
fn code_editor(ui: &mut Ui, doc: &mut ScriptDoc, error_line: Option<usize>) {
    let font = FontId::monospace(13.0);
    let row_height = ui.fonts_mut(|f| f.row_height(&font));
    let mut layouter = |ui: &Ui, text: &dyn egui::TextBuffer, wrap: f32| {
        let mut job = highlight(text.as_str(), font.clone());
        job.wrap.max_width = wrap;
        ui.fonts_mut(|f| f.layout_job(job))
    };
    egui::ScrollArea::both()
        .id_salt("script_code")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.horizontal_top(|ui| {
                let gutter = 44.0;
                let (gutter_rect, _) =
                    ui.allocate_exact_size(egui::vec2(gutter, row_height), egui::Sense::hover());
                let edit = egui::TextEdit::multiline(&mut doc.text)
                    .code_editor()
                    .lock_focus(true)
                    .frame(egui::Frame::NONE)
                    .desired_width(f32::INFINITY)
                    .desired_rows(40)
                    .layouter(&mut layouter)
                    .show(ui);
                // Line numbers beside each line's first row.
                let painter = ui.painter();
                let mut line = 1;
                let mut starts_line = true;
                for row in &edit.galley.rows {
                    if starts_line {
                        let y = edit.galley_pos.y + row.rect().min.y;
                        let wrong = error_line == Some(line);
                        if wrong {
                            painter.rect_filled(
                                egui::Rect::from_min_size(
                                    egui::pos2(gutter_rect.left(), y),
                                    egui::vec2(ui.max_rect().width(), row.rect().height()),
                                ),
                                0.0,
                                theme::ALERT.gamma_multiply(0.15),
                            );
                        }
                        painter.text(
                            egui::pos2(gutter_rect.right() - 8.0, y),
                            egui::Align2::RIGHT_TOP,
                            line.to_string(),
                            font.clone(),
                            if wrong { theme::ALERT } else { theme::LINE },
                        );
                        line += 1;
                    }
                    starts_line = row.ends_with_newline;
                }
            });
        });
}

/// `scripts/<name>.rhai` for a typed name, if it's usable and free.
pub(super) fn script_file(project_dir: &Path, name: &str) -> Result<PathBuf, String> {
    let name = name.trim();
    let name = name.strip_suffix(".rhai").unwrap_or(name);
    if name.is_empty() {
        return Err("TYPE A NAME".into());
    }
    if !crate::project::is_valid_name(name) {
        return Err("USE LETTERS, DIGITS, SPACES, - AND _ ONLY".into());
    }
    let relative = Path::new("scripts").join(format!("{name}.rhai"));
    if project_dir.join(&relative).exists() {
        return Err(format!("{} ALREADY EXISTS", relative.display()).to_uppercase());
    }
    Ok(relative)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_script_is_valid_and_formatted() {
        let text = template("dash");
        assert_eq!(crate::script::syntax_error(&text), None);
        assert_eq!(format::format(&text), text);
    }

    #[test]
    fn saving_formats_only_valid_scripts() {
        let mut doc = ScriptDoc::new("fn update(player,dt){let x=1;}".into());
        assert!(doc.format());
        assert_eq!(
            doc.text,
            "fn update(player, dt) {
    let x = 1;
}\n"
        );
        let mut broken = ScriptDoc::new("fn update(player,dt){let x=;}".into());
        assert!(!broken.format());
        assert_eq!(broken.text, "fn update(player,dt){let x=;}");
    }
}
