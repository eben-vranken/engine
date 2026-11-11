//! The PROJECT workspace: the start screen (recent projects, new, open,
//! packages) and every project-wide setting on one page: name, start scene,
//! render scale, player feel (`player.ron`), dialogue look
//! (with a live preview), flags and the saved game. `game.ron` and
//! `flags.ron` are one document (`project_doc`): edits are undo steps, Ctrl+S
//! here or SAVE ALL writes them, REVERT puts them back; the render scale
//! previews live. Player feel edits are undo steps saved with the scene,
//! as in the inspector.

use super::files::scene_files;
use super::history::Change;
use super::player_panel::player_inspector;
use super::{Editor, EditorOutput, EditorStats, dim, inspector};
use crate::dialogue::{DialogueStyle, Shown};
use crate::player::settings::PlayerSettings;
use crate::render::scale::MAX_FACTOR;
use crate::ui::theme;
use egui::{RichText, Ui};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Start,
    Packages,
    General,
    Startup,
    Rendering,
    PlayerFeel,
    Dialogue,
    Flags,
    Saves,
}

impl Section {
    pub const ALL: [Section; 9] = [
        Section::Start,
        Section::Packages,
        Section::General,
        Section::Startup,
        Section::Rendering,
        Section::PlayerFeel,
        Section::Dialogue,
        Section::Flags,
        Section::Saves,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Section::Start => "START · RECENT",
            Section::Packages => "PACKAGES · EXPORT",
            Section::General => "GENERAL",
            Section::Startup => "STARTUP",
            Section::Rendering => "RENDERING",
            Section::PlayerFeel => "PLAYER FEEL",
            Section::Dialogue => "DIALOGUE",
            Section::Flags => "FLAGS",
            Section::Saves => "SAVES",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Section::Start => "open another game",
            Section::Packages => "copies, the game",
            Section::General => "name",
            Section::Startup => "start scene",
            Section::Rendering => "render scale",
            Section::PlayerFeel => "player.ron",
            Section::Dialogue => "look",
            Section::Flags => "opens LOGIC",
            Section::Saves => "the saved game",
        }
    }
}

/// The page's state while the PROJECT workspace is open (what it edits is
/// the editor's `ProjectDoc`).
pub(super) struct ProjectPage {
    pub section: Section,
    error: Option<String>,
}

impl Editor {
    /// Whether the PROJECT page has `game.ron` changes not yet written.
    pub(super) fn project_unsaved(&self) -> bool {
        self.project.as_ref().is_some_and(|d| d.is_dirty())
    }

    pub(super) fn open_project_page(&mut self, section: Section) {
        if let Some(page) = &mut self.project_page {
            page.section = section;
            return;
        }
        if self.project.is_none() {
            self.project = super::project_doc::ProjectDoc::load(&self.project_dir);
        }
        match &self.project {
            Some(_) => {
                self.project_page = Some(ProjectPage {
                    section,
                    error: None,
                })
            }
            None => self.show_error(format!(
                "COULD NOT READ {}",
                self.project_dir.join("game.ron").display()
            )),
        }
    }

    /// Unsaved `game.ron` changes, for the top bar.
    /// The flags as edited (saved or not), for Play.
    pub fn project_flags(&self) -> Option<crate::graph::flags::FlagDefs> {
        self.project.as_ref().map(|d| d.now.flags.clone())
    }

    pub(super) fn project_changes(&self) -> usize {
        self.project.as_ref().map_or(0, |d| d.changes())
    }

    /// The page: sections on the left, the section's settings in the
    /// middle, a preview on the right when the section has one.
    pub(super) fn project_page_ui(
        &mut self,
        ui: &mut Ui,
        player: &mut PlayerSettings,
        stats: &EditorStats,
        output: &mut EditorOutput,
    ) {
        let Some(section) = self.project_page.as_ref().map(|p| p.section) else {
            return;
        };
        egui::Panel::left("project_sections")
            .resizable(false)
            .exact_size(250.0)
            .show(ui, |ui| {
                ui.add_space(10.0);
                for (heading, sections) in [
                    ("PROJECT", &Section::ALL[..2]),
                    ("SETTINGS", &Section::ALL[2..]),
                ] {
                    ui.label(dim(heading));
                    ui.add_space(4.0);
                    for &s in sections {
                        let text = RichText::new(format!("{}  ", s.title()));
                        let response = ui
                            .selectable_label(s == section, text)
                            .on_hover_text(s.hint().to_uppercase());
                        if response.clicked()
                            && let Some(page) = &mut self.project_page
                        {
                            page.section = s;
                        }
                    }
                    ui.add_space(12.0);
                }
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(
                            "EVERY PROJECT-WIDE SETTING LIVES HERE. GAME.RON AND FLAGS.RON ARE ONE DOCUMENT: EDITS UNDO, CTRL+S SAVES.",
                        )
                        .small()
                        .color(theme::AMBER_DIM),
                    );
                });
            });
        if section == Section::Dialogue {
            egui::Panel::right("project_preview")
                .resizable(true)
                .default_size(520.0)
                .show(ui, |ui| {
                    ui.add_space(8.0);
                    ui.label(RichText::new("LIVE PREVIEW").strong());
                    let style = self
                        .project
                        .as_ref()
                        .map(|d| d.now.game.dialogue.clone())
                        .unwrap_or_default();
                    dialogue_preview(ui, &style);
                });
        }
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink(false)
                .show(ui, |ui| {
                    ui.add_space(8.0);
                    ui.label(RichText::new(section.title()).heading().strong());
                    ui.add_space(8.0);
                    match section {
                        Section::Start => self.start_section(ui, output),
                        Section::Packages => self.packages_section(ui),
                        Section::PlayerFeel => self.feel_section(ui, player, output),
                        Section::Flags => {
                            ui.label(dim("FLAGS ARE EDITED NEXT TO THE GRAPHS THAT USE THEM."));
                            ui.add_space(6.0);
                            if ui.button("OPEN LOGIC ›").clicked() {
                                self.go(super::commands::Workspace::Logic, output);
                            }
                        }
                        Section::Saves => self.saves_section(ui),
                        _ => self.settings_section(ui, section, stats, output),
                    }
                });
        });
    }

    fn start_section(&mut self, ui: &mut Ui, output: &mut EditorOutput) {
        ui.label(dim(&format!(
            "OPEN NOW: {}",
            self.project_dir.display().to_string().to_uppercase()
        )));
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let waiting = self.file_dialog_open();
            for (text, purpose) in [
                ("NEW PROJECT…", super::file_dialogs::Purpose::NewProject),
                ("OPEN PROJECT…", super::file_dialogs::Purpose::OpenProject),
                ("OPEN PACKAGE…", super::file_dialogs::Purpose::OpenPackage),
            ] {
                if ui.add_enabled(!waiting, egui::Button::new(text)).clicked() {
                    self.ask_file(purpose);
                }
            }
        });
        ui.add_space(12.0);
        ui.label(RichText::new("RECENT").strong());
        ui.add_space(4.0);
        let recent = crate::recent::load();
        let now = crate::recent::now();
        if recent.is_empty() {
            ui.label(dim("NO RECENT PROJECTS"));
        }
        let mut open = None;
        egui::Grid::new("recent_projects")
            .num_columns(3)
            .spacing([18.0, 6.0])
            .show(ui, |ui| {
                for project in recent {
                    let missing = !project.dir.join("game.ron").is_file();
                    ui.label(project.name.to_uppercase());
                    ui.label(dim(&crate::recent::ago(project.opened, now).to_uppercase()));
                    if missing {
                        ui.label(RichText::new("MISSING").color(theme::ALERT));
                    } else if ui
                        .small_button("OPEN")
                        .on_hover_text(project.dir.display().to_string())
                        .clicked()
                    {
                        open = Some(project.dir.clone());
                    }
                    ui.end_row();
                }
            });
        if let Some(dir) = open
            && let Some(switch) = self.switch(super::file_dialogs::Switch::Open(dir))
        {
            self.switch_project(switch, output);
        }
    }

    fn packages_section(&mut self, ui: &mut Ui) {
        ui.label(dim(
            "A PACKAGE IS THE WHOLE PROJECT IN ONE .GAMEPACK FILE, TO OPEN IN ANOTHER EDITOR.",
        ));
        ui.label(dim("ONLY SAVED FILES GO IN; CACHES STAY OUT."));
        ui.add_space(8.0);
        let waiting = self.file_dialog_open();
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!waiting, egui::Button::new("EXPORT PACKAGE…"))
                .clicked()
            {
                self.ask_file(super::file_dialogs::Purpose::ExportPackage);
            }
            if ui
                .add_enabled(!waiting, egui::Button::new("OPEN PACKAGE…"))
                .clicked()
            {
                self.ask_file(super::file_dialogs::Purpose::OpenPackage);
            }
        });
        ui.add_space(12.0);
        ui.label(dim(
            "THE GAME IS ONE EXECUTABLE FOR PLAYERS: THIS ENGINE WITH THE PROJECT INSIDE IT.",
        ));
        ui.label(dim(
            "IT PLAYS FROM THE START SCENE, WITHOUT THE EDITOR OR THE TEST TOOLS.",
        ));
        ui.add_space(8.0);
        if ui
            .add_enabled(!waiting, egui::Button::new("EXPORT GAME…"))
            .clicked()
        {
            self.ask_file(super::file_dialogs::Purpose::ExportGame);
        }
    }

    fn feel_section(
        &mut self,
        ui: &mut Ui,
        player: &mut PlayerSettings,
        output: &mut EditorOutput,
    ) {
        let before = player.clone();
        player_inspector(ui, player, &self.project_dir);
        if *player != before {
            self.begin_player_edit(before, player);
            output.changed.push(Change::Player);
        }
    }

    fn saves_section(&mut self, ui: &mut Ui) {
        match crate::save::file(&self.project_dir) {
            Some(file) => {
                ui.label(dim(
                    "ONE SAVED GAME PER PROJECT, IN YOUR USER FOLDER (NEVER IN THE PROJECT).",
                ));
                ui.add_space(6.0);
                ui.label(file.display().to_string());
                ui.add_space(6.0);
                let exists = file.is_file();
                ui.label(if exists {
                    RichText::new("[A SAVED GAME EXISTS]")
                } else {
                    dim("NO SAVED GAME YET")
                });
                ui.add_space(6.0);
                if ui
                    .add_enabled(exists, egui::Button::new("DELETE THE SAVED GAME"))
                    .on_hover_text("CONTINUE SAVE AND THE GAME'S START BEGIN FRESH AGAIN")
                    .clicked()
                {
                    match std::fs::remove_file(&file) {
                        Ok(()) => self.show_message("DELETED THE SAVED GAME".into()),
                        Err(err) => self.show_error(err.to_string()),
                    }
                }
            }
            None => {
                ui.label(dim(
                    "THIS SYSTEM HAS NO USER CONFIG FOLDER, SO GAMES CAN'T BE SAVED.",
                ));
            }
        }
    }

    /// GENERAL, STARTUP, RENDERING, DIALOGUE: `game.ron` values.
    fn settings_section(
        &mut self,
        ui: &mut Ui,
        section: Section,
        stats: &EditorStats,
        output: &mut EditorOutput,
    ) {
        let scenes = scene_files(&self.project_dir);
        let (Some(page), Some(doc)) = (&mut self.project_page, &mut self.project) else {
            return;
        };
        let settings = &mut doc.now.game;
        let scale_before = settings.render_scale;
        egui::Grid::new(("settings", section.title()))
            .num_columns(2)
            .spacing([16.0, 8.0])
            .show(ui, |ui| match section {
                Section::General => {
                    ui.label(dim("NAME"));
                    ui.text_edit_singleline(&mut settings.name);
                    ui.end_row();
                }
                Section::Startup => {
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
                    ui.end_row();
                }
                Section::Rendering => {
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
                    ui.label("");
                    ui.label(dim(&format!(
                        "NOW ×{}: {}×{} INTERNAL. [ AND ] STILL CHANGE IT WHILE RUNNING.",
                        stats.factor, stats.internal[0], stats.internal[1]
                    )));
                    ui.end_row();
                }
                Section::Dialogue => {
                    ui.label(dim("HOW LINES AND REPLIES LOOK IN THIS GAME"));
                    ui.end_row();
                }
                _ => {}
            });
        if section == Section::Dialogue {
            ui.add_space(6.0);
            inspector::fields(ui, &mut settings.dialogue, &inspector::Names::default());
            ui.add_space(6.0);
            if ui.small_button("RESET TO THE AMBER DEFAULT").clicked() {
                settings.dialogue = DialogueStyle::default();
            }
        }
        if settings.render_scale != scale_before {
            output.render_scale = Some(settings.render_scale); // a live preview
        }
        if let Some(error) = &page.error {
            ui.add_space(8.0);
            ui.label(RichText::new(error).color(theme::ALERT));
        }
    }

    /// Saves the project's settings (`game.ron`, `flags.ron`).
    pub(super) fn save_project_settings(&mut self, output: &mut EditorOutput) {
        let dir = self.project_dir.clone();
        let Some(doc) = &mut self.project else {
            return;
        };
        let result = doc.save(&dir);
        let name = doc.now.game.name.clone();
        match result {
            Ok(()) => {
                log::info!("saved game.ron and flags.ron");
                if let Some(page) = &mut self.project_page {
                    page.error = None;
                }
                self.game_name = name.clone();
                output.rename = Some(name);
                self.show_message("SAVED GAME.RON AND FLAGS.RON".into());
                self.index.flags_unsaved = false;
                self.index.refresh(&dir, true);
            }
            Err(err) => {
                if let Some(page) = &mut self.project_page {
                    page.error = Some(err.clone());
                }
                self.show_error(err);
            }
        }
    }

    pub(super) fn revert_project_settings(&mut self, output: &mut EditorOutput) {
        if let Some(doc) = &mut self.project {
            let before = doc.now.game.render_scale;
            doc.revert();
            if doc.now.game.render_scale != before {
                output.render_scale = Some(doc.now.game.render_scale);
            }
            self.index.flags = doc.now.flags.clone();
        }
        if let Some(page) = &mut self.project_page {
            page.error = None;
        }
    }
}

/// The game's dialogue box over a stand-in view, with a sample line typing
/// out (it loops).
fn dialogue_preview(ui: &mut Ui, style: &DialogueStyle) {
    let size = egui::vec2(
        ui.available_width(),
        (ui.available_height() - 40.0).max(200.0),
    );
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(14, 11, 8));
    painter.rect_stroke(
        rect,
        0.0,
        egui::Stroke::new(1.0, theme::LINE),
        egui::StrokeKind::Inside,
    );
    let text = "Who goes there? The gate has been shut since the lamps went out.";
    let time = ui.input(|i| i.time) as f32;
    let speed = style.typing_speed.max(1.0);
    let cycle = text.chars().count() as f32 / speed + 2.0;
    let (shown, complete) = crate::dialogue::typed(text, time % cycle, style.typing_speed);
    let replies = ["A friend.".to_string(), "Nobody.".to_string()];
    crate::dialogue::draw(
        ui,
        rect,
        style,
        false,
        &Shown {
            speaker: "???",
            text: shown,
            complete,
            replies: &replies,
            choice: 0,
        },
    );
    ui.ctx().request_repaint();
    ui.label(dim(
        "A SAMPLE LINE; THE GAME'S OWN FONT SHOWS WHEN PLAYING.",
    ));
}
