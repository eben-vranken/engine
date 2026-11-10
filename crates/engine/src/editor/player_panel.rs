//! The game's player settings and footsteps, shown under Player Start.

use super::files::sound_files;
use super::history::Edit;
use super::widgets::on_off;
use super::{Editor, dim, inspector, section};
use crate::player::settings::{Footsteps, PlayerSettings, RandomOrder};
use crate::player::tuning;
use crate::scene::{Component, Scene};
use crate::ui::theme;
use egui::{Align, Layout, RichText, Ui};
use std::path::Path;

impl Editor {
    /// Starts (or continues) an edit of the player settings; `before` is
    /// how they were before this frame's change.
    pub(super) fn begin_player_edit(&mut self, before: PlayerSettings, now: &PlayerSettings) {
        match &mut self.player_pending {
            Some((_, after)) => **after = now.clone(),
            None => self.player_pending = Some((Box::new(before), Box::new(now.clone()))),
        }
    }

    /// Whether the inspector shows the player settings (Player Start is selected).
    pub fn shows_player(&self, scene: &Scene) -> bool {
        self.selected
            .and_then(|id| scene.object(id))
            .is_some_and(|o| o.components.contains(&Component::PlayerStart))
    }

    /// Adds a sound file to the footsteps (and turns the default steps
    /// off), as one undo step.
    pub fn add_footstep(&mut self, scene: &Scene, player: &mut PlayerSettings, path: String) {
        self.commit_pending(scene);
        let before = player.clone();
        player.footsteps.use_default = false;
        player.footsteps.clips.push(path);
        self.history.record(Edit::SetPlayer {
            before: Box::new(before),
            after: Box::new(player.clone()),
        });
    }
}

/// The game's player settings (its `player.ron`), shown under Player Start:
/// movement, jump, body and camera as in the F1 tuning panel, then the
/// footsteps.
pub(super) fn player_inspector(ui: &mut Ui, player: &mut PlayerSettings, project_dir: &Path) {
    section(ui, "PLAYER");
    ui.label(dim(
        "THE GAME'S PLAYER.RON: SHARED BY EVERY SCENE, SAVED WITH THE SCENE",
    ));
    tuning::sections(ui, player);
    section(ui, "FOOTSTEPS");
    footsteps_inspector(ui, &mut player.footsteps, project_dir);
}

pub(super) fn footsteps_inspector(ui: &mut Ui, steps: &mut Footsteps, project_dir: &Path) {
    ui.horizontal(|ui| {
        inspector::field_label(ui, "VOLUME");
        ui.add(
            egui::DragValue::new(&mut steps.volume)
                .range(0.0..=2.0)
                .speed(0.01)
                .max_decimals(2),
        );
        ui.label(dim("1 = AS RECORDED"));
    });
    on_off(ui, "USE DEFAULT", &mut steps.use_default);
    if steps.use_default {
        ui.label(dim("THE ENGINE'S STEPS, ONE PER FLOOR MATERIAL"));
        return;
    }

    if steps.clips.is_empty() {
        ui.label(dim(
            "NO SOUNDS YET: ADD ONE, OR DROP .WAV FILES ON THE WINDOW",
        ));
        ui.label(dim("UNTIL THEN THE DEFAULT STEPS PLAY"));
    }
    enum ListAction {
        Up(usize),
        Down(usize),
        Remove(usize),
    }
    let mut action = None;
    let count = steps.clips.len();
    for (i, clip) in steps.clips.iter().enumerate() {
        ui.horizontal(|ui| {
            ui.label(dim(&format!("{:>2}", i + 1)));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.small_button("X").on_hover_text("REMOVE").clicked() {
                    action = Some(ListAction::Remove(i));
                }
                if ui
                    .add_enabled(i + 1 < count, egui::Button::new("↓").small())
                    .clicked()
                {
                    action = Some(ListAction::Down(i));
                }
                if ui
                    .add_enabled(i > 0, egui::Button::new("↑").small())
                    .clicked()
                {
                    action = Some(ListAction::Up(i));
                }
                let text = if project_dir.join(clip).is_file() {
                    RichText::new(clip)
                } else {
                    RichText::new(format!("{clip} (MISSING)")).color(theme::ALERT)
                };
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    ui.add(egui::Label::new(text).truncate());
                });
            });
        });
    }
    match action {
        Some(ListAction::Up(i)) => steps.clips.swap(i, i - 1),
        Some(ListAction::Down(i)) => steps.clips.swap(i, i + 1),
        Some(ListAction::Remove(i)) => {
            steps.clips.remove(i);
        }
        None => {}
    }
    ui.menu_button("ADD SOUND", |ui| {
        let files = sound_files(project_dir);
        if files.is_empty() {
            ui.label(dim("NO .WAV FILES IN THE PROJECT"));
        }
        egui::ScrollArea::vertical()
            .max_height(320.0)
            .show(ui, |ui| {
                for file in files {
                    if ui.button(&file).clicked() {
                        steps.clips.push(file);
                        ui.close();
                    }
                }
            });
    });

    ui.add_space(6.0);
    on_off(ui, "RANDOMIZE", &mut steps.randomize);
    if !steps.randomize {
        ui.label(dim("IN LIST ORDER: 1, 2, 3, 1, 2, 3..."));
        return;
    }
    let name = |order| match order {
        RandomOrder::True => "TRUE RANDOM",
        RandomOrder::Smart => "SMART RANDOM",
    };
    ui.horizontal(|ui| {
        inspector::field_label(ui, "ORDER");
        egui::ComboBox::from_id_salt("footstep_order")
            .selected_text(name(steps.random))
            .show_ui(ui, |ui| {
                for order in [RandomOrder::True, RandomOrder::Smart] {
                    ui.selectable_value(&mut steps.random, order, name(order));
                }
            });
    });
    ui.label(dim(match steps.random {
        RandomOrder::True => "ANY SOUND EACH STEP; THE SAME ONE CAN REPEAT",
        RandomOrder::Smart => {
            "NO RECENT REPEATS, AND EACH STEP SLIGHTLY VARIED IN PITCH AND LOUDNESS"
        }
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::testing::*;

    #[test]
    fn a_player_settings_drag_is_one_undo_step() {
        let scene = boxes(1);
        let mut scene_after = scene.clone();
        let mut editor = editor();
        let mut player = PlayerSettings::default();
        // As the inspector does on each frame of a drag.
        for speed in [5.0, 6.0, 7.0] {
            let before = player.clone();
            player.movement.walk_speed = speed;
            editor.begin_player_edit(before, &player);
        }
        assert!(editor.is_dirty());
        assert!(editor.commit_pending(&scene));
        editor.add_footstep(&scene, &mut player, "audio/steps/a.wav".into());
        assert!(!player.footsteps.use_default);

        editor.undo(&mut scene_after, &mut player);
        assert!(player.footsteps.use_default && player.footsteps.clips.is_empty());
        assert_eq!(player.movement.walk_speed, 7.0);
        editor.undo(&mut scene_after, &mut player);
        assert_eq!(player, PlayerSettings::default());
        assert_eq!(scene_after, scene);
    }
}
