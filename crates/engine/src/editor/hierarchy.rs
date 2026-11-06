//! The scene panel: objects grouped by zone, search, and zone edits
//! (add, rename, delete).

use super::history::{Change, Edit};
use super::{Editor, EditorOutput, SettingsTarget, dim, inspector};
use crate::scene::{Component, Object, Scene, Zone};
use crate::ui::theme;
use egui::{Align, Layout, RichText, Ui};

impl Editor {
    pub(super) fn hierarchy(&mut self, ui: &mut Ui, scene: &mut Scene, output: &mut EditorOutput) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("SCENE").strong());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.small_button("+ ZONE").clicked() {
                    output.changed.push(self.add_zone(scene));
                }
            });
        });
        ui.label(dim(&format!(
            "{} OBJECTS IN {} ZONES  (RIGHT-CLICK A ZONE)",
            scene.objects.len(),
            scene.zones.len()
        )));
        ui.add_space(4.0);
        // Right to left: the X first, the field fills what's left (never
        // negative: the panel can be very narrow on its first frame).
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .add_enabled(!self.search.is_empty(), egui::Button::new("X").small())
                    .on_hover_text("CLEAR THE SEARCH")
                    .clicked()
                {
                    self.search.clear();
                }
                ui.add(
                    egui::TextEdit::singleline(&mut self.search)
                        .hint_text("SEARCH NAME, ZONE OR COMPONENT")
                        .desired_width(ui.available_width().max(0.0)),
                );
            });
        });
        let query = self.search.trim().to_lowercase();
        let searching = !query.is_empty();
        ui.add_space(4.0);
        let mut clicked = None;
        let mut zone_clicked = None;
        let mut zone_action = None;
        let environment = self.settings == Some(SettingsTarget::Environment);
        if ui
            .selectable_label(environment, picked("ENVIRONMENT", environment))
            .clicked()
        {
            zone_clicked = Some(SettingsTarget::Environment);
        }
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
                    // A zone whose name matches shows all its objects.
                    let zone_matches = zone.name.to_lowercase().contains(&query);
                    let total = members.len();
                    let members: Vec<(usize, &Object)> = if zone_matches {
                        members
                    } else {
                        members
                            .into_iter()
                            .filter(|(_, o)| matches_search(o, &query))
                            .collect()
                    };
                    if searching && members.is_empty() && !zone_matches {
                        continue;
                    }
                    let shown = if members.len() == total {
                        total.to_string()
                    } else {
                        format!("{}/{total}", members.len())
                    };
                    let header = format!("{} [{shown}]", zone.name.to_uppercase());
                    let reveal = self.reveal_selection
                        && members.iter().any(|(i, _)| Some(*i) == self.selected);
                    if let Some((old, typed)) = &mut self.renaming
                        && *old == zone.name
                    {
                        let response = ui.text_edit_singleline(typed);
                        if !response.has_focus() && !response.lost_focus() {
                            response.request_focus();
                        }
                        if response.lost_focus() {
                            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                            zone_action = Some(if enter {
                                ZoneAction::Rename(old.clone(), typed.clone())
                            } else {
                                ZoneAction::CancelRename
                            });
                        }
                        continue;
                    }
                    let empty = total == 0;
                    let chosen = self.settings == Some(SettingsTarget::Zone(zone.name.clone()));
                    let response = egui::CollapsingHeader::new(picked(&header, chosen))
                        .id_salt(&zone.name)
                        .open((reveal || searching).then_some(true))
                        .show(ui, |ui| {
                            for (index, object) in members {
                                let selected = self.selected == Some(index);
                                let response =
                                    ui.selectable_label(selected, picked(&object.name, selected));
                                if response.clicked() {
                                    clicked = Some(index);
                                }
                                if selected && reveal {
                                    response.scroll_to_me(Some(Align::Center));
                                }
                            }
                        });
                    if response.header_response.clicked() {
                        zone_clicked = Some(SettingsTarget::Zone(zone.name.clone()));
                    }
                    response.header_response.context_menu(|ui| {
                        if ui.button("RENAME").clicked() {
                            zone_action = Some(ZoneAction::StartRename(zone.name.clone()));
                            ui.close();
                        }
                        if ui
                            .add_enabled(empty, egui::Button::new("DELETE"))
                            .on_disabled_hover_text("ONLY EMPTY ZONES")
                            .clicked()
                        {
                            zone_action = Some(ZoneAction::Delete(zone.name.clone()));
                            ui.close();
                        }
                    });
                }
                if searching
                    && !scene.zones.iter().any(|z| {
                        z.name.to_lowercase().contains(&query)
                            || scene
                                .objects
                                .iter()
                                .any(|o| o.zone == z.name && matches_search(o, &query))
                    })
                {
                    ui.label(dim("NOTHING MATCHES"));
                }
            });
        self.reveal_selection = false;
        if let Some(index) = clicked {
            self.select(scene, Some(index));
        }
        if let Some(target) = zone_clicked {
            self.select(scene, None);
            self.settings = Some(target);
        }
        let change = match zone_action {
            Some(ZoneAction::StartRename(name)) => {
                self.renaming = Some((name.clone(), name));
                None
            }
            Some(ZoneAction::CancelRename) => {
                self.renaming = None;
                None
            }
            Some(ZoneAction::Rename(old, new)) => {
                self.renaming = None;
                if new.trim() != old {
                    let change = self.rename_zone(scene, &old, &new);
                    if change.is_none() {
                        self.show_error(format!("CAN'T RENAME TO \"{}\"", new.trim()));
                    }
                    change
                } else {
                    None
                }
            }
            Some(ZoneAction::Delete(name)) => self.delete_zone(scene, &name),
            None => None,
        };
        if let Some(change) = change {
            output.changed.push(change);
            output.settled = true;
        }
    }
    /// Adds an empty zone named `zone`, `zone 2`... and starts renaming it.
    pub(super) fn add_zone(&mut self, scene: &mut Scene) -> Change {
        let taken = |name: &str| scene.zones.iter().any(|z| z.name == name);
        let name = std::iter::once("zone".to_owned())
            .chain((2..).map(|n| format!("zone {n}")))
            .find(|name| !taken(name))
            .expect("some number is free");
        let mut after = scene.clone();
        after.zones.push(Zone::new(name.clone()));
        self.renaming = Some((name.clone(), name));
        self.push_scene(scene, after)
    }

    /// Renames a zone, and every object and portal that refers to it.
    /// `None` if the new name is empty, taken or unchanged.
    pub(super) fn rename_zone(
        &mut self,
        scene: &mut Scene,
        old: &str,
        new: &str,
    ) -> Option<Change> {
        let new = new.trim();
        if new.is_empty() || scene.zones.iter().any(|z| z.name == new) {
            return None;
        }
        let mut after = scene.clone();
        for zone in after.zones.iter_mut().filter(|z| z.name == old) {
            zone.name = new.to_owned();
        }
        for object in &mut after.objects {
            if object.zone == old {
                object.zone = new.to_owned();
            }
            for component in &mut object.components {
                if let Component::Portal(portal) = component
                    && portal.to.0 == old
                {
                    portal.to.0 = new.to_owned();
                }
            }
        }
        Some(self.push_scene(scene, after))
    }

    /// Removes a zone with no objects in it.
    pub(super) fn delete_zone(&mut self, scene: &mut Scene, name: &str) -> Option<Change> {
        if scene.objects.iter().any(|o| o.zone == name) {
            return None;
        }
        let mut after = scene.clone();
        after.zones.retain(|z| z.name != name);
        Some(self.push_scene(scene, after))
    }

    /// Replaces the whole scene as one undo step.
    pub(super) fn push_scene(&mut self, scene: &mut Scene, after: Scene) -> Change {
        self.commit_pending(scene);
        let before = Box::new(scene.clone());
        self.push(
            scene,
            Edit::SetScene {
                before,
                after: Box::new(after),
            },
        )
    }
}

/// Whether an object's name or one of its components (e.g. "light",
/// "door") contains `query` (lowercase), ignoring case.
pub(super) fn matches_search(object: &Object, query: &str) -> bool {
    object.name.to_lowercase().contains(query)
        || object
            .components
            .iter()
            .any(|c| inspector::title(c).to_lowercase().contains(query))
}

/// A zone edit picked in the hierarchy, applied after drawing it.
pub(super) enum ZoneAction {
    StartRename(String),
    CancelRename,
    /// Old name, new name.
    Rename(String, String),
    Delete(String),
}

/// A row's text in the hierarchy: inverted (amber background, text in the
/// background color) when it's the one being inspected.
fn picked(text: &str, selected: bool) -> RichText {
    let text = RichText::new(text);
    if selected {
        text.color(theme::BACKGROUND).background_color(theme::AMBER)
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::testing::*;
    use crate::player::settings::PlayerSettings;
    use crate::scene::Portal;

    #[test]
    fn search_matches_names_and_components_ignoring_case() {
        let scene = boxes(1);
        let object = &scene.objects[0];
        assert!(matches_search(object, &object.name.to_lowercase()));
        assert!(matches_search(object, "primitive"));
        assert!(!matches_search(object, "no such thing"));
    }
    #[test]
    fn renaming_a_zone_moves_its_objects_and_portals_and_undoes() {
        let mut scene = boxes(2);
        scene.zones.push(crate::scene::Zone::new("hall"));
        scene.objects[1].components = vec![Component::Portal(Portal {
            to: crate::scene::ZoneRef("test".into()),
        })];
        scene.objects[1].zone = "hall".into();
        let original = scene.clone();
        let mut editor = editor();

        assert!(
            editor.rename_zone(&mut scene, "test", "hall").is_none(),
            "taken"
        );
        assert_eq!(
            editor.rename_zone(&mut scene, "test", " cellar "),
            Some(Change::All)
        );
        assert_eq!(scene.zones[0].name, "cellar");
        assert_eq!(scene.objects[0].zone, "cellar");
        assert_eq!(
            scene.objects[1].components[0],
            Component::Portal(Portal {
                to: crate::scene::ZoneRef("cellar".into())
            })
        );
        assert!(
            editor.delete_zone(&mut scene, "cellar").is_none(),
            "not empty"
        );
        editor.undo(&mut scene, &mut PlayerSettings::default());
        assert_eq!(scene, original);

        editor.add_zone(&mut scene);
        assert_eq!(scene.zones.last().unwrap().name, "zone");
        assert!(editor.delete_zone(&mut scene, "zone").is_some());
        assert_eq!(scene, original);
    }
}
