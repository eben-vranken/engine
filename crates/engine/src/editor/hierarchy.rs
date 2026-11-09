//! The outliner: objects grouped by zone with a type tag each, filter
//! chips, search, and zone edits (add, rename, delete).

use super::history::{Change, Edit};
use super::{Editor, EditorOutput, SettingsTarget, dialog_heading, dim, inspector};
use crate::scene::{Component, Object, Scene, Zone};
use crate::ui::theme;
use egui::{Align, Layout, RichText, Ui};

impl Editor {
    pub(super) fn hierarchy(&mut self, ui: &mut Ui, scene: &mut Scene, output: &mut EditorOutput) {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for filter in Filter::ALL {
                let on = self.outliner_filter == filter;
                let text = if on {
                    RichText::new(filter.title())
                        .small()
                        .color(theme::BACKGROUND)
                } else {
                    RichText::new(filter.title())
                        .small()
                        .color(theme::AMBER_DIM)
                };
                let mut chip = egui::Button::new(text).small();
                if on {
                    chip = chip.fill(theme::AMBER);
                }
                if ui.add(chip).clicked() {
                    self.outliner_filter = filter;
                }
            }
        });
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
                        .hint_text("NAME, ZONE OR COMPONENT; MODEL > PART")
                        .desired_width(ui.available_width().max(0.0)),
                );
            });
        });
        let query = Query::parse(&self.search);
        let filter = self.outliner_filter;
        let searching = !query.is_empty() || filter != Filter::All;
        // MODEL > PART: every matching part, to pick in one go.
        let found = if query.part.is_some() {
            search_parts(scene, &self.model_parts, &query)
        } else {
            Vec::new()
        };
        if !found.is_empty() {
            let text = format!("SELECT ALL {}", found.len());
            if ui
                .small_button(text)
                .on_hover_text("SELECT EVERY PART FOUND, TO EDIT THEM TOGETHER")
                .clicked()
            {
                let settled = self.select(scene, Some(found[0].object));
                output.settled |= settled;
                self.selected_part = found[0].part.clone();
                self.also = found[1..].to_vec();
            }
        }
        let mut shown_any = false;
        ui.add_space(4.0);
        let mut clicked = None;
        let mut zone_clicked = None;
        let mut zone_action = None;
        let environment = self.settings == Some(SettingsTarget::Environment);
        if ui
            .selectable_label(environment, picked("SCENE SETTINGS", environment))
            .on_hover_text("SUN, SKY, FOG, GRADE, BLOOM AND BAKE QUALITY FOR THE WHOLE SCENE")
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
                    let zone_matches = query.part.is_none()
                        && filter == Filter::All
                        && zone.name.to_lowercase().contains(&query.object);
                    let total = members.len();
                    let members: Vec<(usize, &Object)> = if zone_matches {
                        members
                    } else {
                        members
                            .into_iter()
                            .filter(|(_, o)| {
                                filter.shows(o)
                                    && query.shows(o, &zone.name, &model_parts(o, &self.model_parts))
                            })
                            .collect()
                    };
                    if searching && members.is_empty() && !zone_matches {
                        continue;
                    }
                    shown_any = true;
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
                    let chosen = self.settings == Some(SettingsTarget::Zone(zone.name.clone()));
                    let response = egui::CollapsingHeader::new(picked(&header, chosen))
                        .id_salt(&zone.name)
                        .open((reveal || searching).then_some(true))
                        .show(ui, |ui| {
                            for (index, object) in members {
                                let selected = self.selected == Some(index);
                                let parts = model_parts(object, &self.model_parts);
                                let whole = self.is_picked(index, None);
                                if parts.len() < 2 && query.part.is_none() {
                                    let response = object_row(ui, whole, object);
                                    if response.clicked() {
                                        clicked = Some((index, None));
                                    }
                                    if selected && reveal {
                                        response.scroll_to_me(Some(Align::Center));
                                    }
                                    continue;
                                }
                                // A model made of parts opens to show them.
                                let id = ui.make_persistent_id(("object_parts", &object.name, index));
                                let mut state =
                                    egui::collapsing_header::CollapsingState::load_with_default_open(
                                        ui.ctx(),
                                        id,
                                        false,
                                    );
                                let part_picked =
                                    self.picks().iter().any(|p| p.object == index && p.part.is_some());
                                if ((reveal || selected) && part_picked) || query.part.is_some() {
                                    state.set_open(true);
                                }
                                let parts: Vec<String> =
                                    parts.into_iter().filter(|p| query.part_matches(p)).collect();
                                let header = state.show_header(ui, |ui| {
                                    let response = object_row(ui, whole, object);
                                    if response.clicked() {
                                        clicked = Some((index, None));
                                    }
                                    if selected && reveal {
                                        response.scroll_to_me(Some(Align::Center));
                                    }
                                });
                                header.body(|ui| {
                                    for part in parts {
                                        let chosen = self.is_picked(index, Some(&part));
                                        let own = has_own_settings(object, &part);
                                        let text = if own {
                                            format!("{part} *")
                                        } else {
                                            part.clone()
                                        };
                                        if ui
                                            .selectable_label(chosen, picked(&text, chosen))
                                            .on_hover_text(if own {
                                                "A PART WITH SETTINGS OF ITS OWN"
                                            } else {
                                                "A PART OF THE MODEL"
                                            })
                                            .clicked()
                                        {
                                            clicked = Some((index, Some(part)));
                                        }
                                    }
                                });
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
                        if ui.button("DELETE").clicked() {
                            zone_action = Some(ZoneAction::Delete(zone.name.clone()));
                            ui.close();
                        }
                    });
                }
                if searching && !shown_any {
                    ui.label(dim("NOTHING MATCHES"));
                }
            });
        self.reveal_selection = false;
        self.zone_delete_dialog(ui, scene, output);
        if let Some((index, part)) = clicked {
            if ui.input(|i| i.modifiers.shift) {
                self.toggle_pick(
                    scene,
                    super::Pick {
                        object: index,
                        part,
                    },
                );
            } else {
                self.select(scene, Some(index));
                self.selected_part = part;
            }
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
            Some(ZoneAction::Delete(name)) => {
                if scene.objects.iter().any(|o| o.zone == name) {
                    let target = scene
                        .zones
                        .iter()
                        .map(|z| z.name.clone())
                        .find(|z| *z != name)
                        .unwrap_or_default();
                    self.zone_delete = Some((name, target));
                    None
                } else {
                    self.delete_zone(scene, &name, None)
                }
            }
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

    /// Removes a zone. Its objects move to `move_to` (and portals into it
    /// lead there instead), or are deleted with it if that is `None`.
    /// `None` if either zone doesn't exist or they are the same.
    pub(super) fn delete_zone(
        &mut self,
        scene: &mut Scene,
        name: &str,
        move_to: Option<&str>,
    ) -> Option<Change> {
        let exists = |zone: &str| scene.zones.iter().any(|z| z.name == zone);
        if !exists(name) || move_to.is_some_and(|to| to == name || !exists(to)) {
            return None;
        }
        let mut after = scene.clone();
        after.zones.retain(|z| z.name != name);
        match move_to {
            Some(to) => {
                for object in &mut after.objects {
                    if object.zone == name {
                        object.zone = to.to_owned();
                    }
                    for component in &mut object.components {
                        if let Component::Portal(portal) = component
                            && portal.to.0 == name
                        {
                            portal.to.0 = to.to_owned();
                        }
                    }
                }
            }
            None => after.objects.retain(|o| o.zone != name),
        }
        self.selected = None;
        if self.settings == Some(SettingsTarget::Zone(name.to_owned())) {
            self.settings = None;
        }
        Some(self.push_scene(scene, after))
    }

    /// Asks what happens to a zone's objects before deleting it: delete
    /// them too, or move them to another zone.
    fn zone_delete_dialog(&mut self, ui: &Ui, scene: &mut Scene, output: &mut EditorOutput) {
        let Some((zone, target)) = &mut self.zone_delete else {
            return;
        };
        let members: Vec<&Object> = scene.objects.iter().filter(|o| o.zone == *zone).collect();
        let has_start = members
            .iter()
            .any(|o| o.components.contains(&Component::PlayerStart));
        let others: Vec<String> = scene
            .zones
            .iter()
            .map(|z| z.name.clone())
            .filter(|z| z != zone)
            .collect();
        let mut decision = None;
        egui::Modal::new(egui::Id::unique("delete_zone")).show(ui.ctx(), |ui| {
            let count = members.len();
            let objects = if count == 1 { "OBJECT" } else { "OBJECTS" };
            dialog_heading(
                ui,
                &format!("DELETE ZONE {}", zone.to_uppercase()),
                &format!("IT HOLDS {count} {objects}."),
            );
            ui.horizontal(|ui| {
                let moved = ui
                    .add_enabled(!others.is_empty(), egui::Button::new("MOVE THEM TO"))
                    .on_disabled_hover_text("THERE IS NO OTHER ZONE")
                    .clicked();
                if moved {
                    decision = Some(Some(target.clone()));
                }
                egui::ComboBox::from_id_salt("delete_zone_target")
                    .selected_text(target.to_uppercase())
                    .show_ui(ui, |ui| {
                        for other in &others {
                            ui.selectable_value(target, other.clone(), other.to_uppercase());
                        }
                    });
            });
            if ui.button("DELETE THEM TOO").clicked() {
                decision = Some(None);
            }
            if has_start {
                ui.label(
                    RichText::new("THE PLAYER START IS ONE OF THEM")
                        .small()
                        .color(theme::ALERT),
                );
            }
            ui.add_space(8.0);
            if ui.button("CANCEL").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                decision = Some(Some(String::new())); // cancelled below
            }
        });
        let Some(move_to) = decision else {
            return;
        };
        let zone = zone.clone();
        self.zone_delete = None;
        if move_to.as_deref() == Some("") {
            return; // cancelled
        }
        if let Some(change) = self.delete_zone(scene, &zone, move_to.as_deref()) {
            output.changed.push(change);
            output.settled = true;
        }
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

/// The parts of the object's model, if it is one (from the loaded file).
fn model_parts(object: &Object, parts: &super::ModelParts) -> Vec<String> {
    object
        .components
        .iter()
        .find_map(|c| match c {
            Component::Model(m) => parts.get(&m.path),
            _ => None,
        })
        .map(|parts| parts.iter().map(|(name, _)| name.clone()).collect())
        .unwrap_or_default()
}

/// Whether the object's model gives `part` settings of its own.
fn has_own_settings(object: &Object, part: &str) -> bool {
    object.components.iter().any(|c| match c {
        Component::Model(m) => m.part(part).is_some(),
        _ => false,
    })
}

/// What's typed in the search bar. `candle > sphere` finds the parts
/// named like `sphere` in every object named like `candle` (`candle 3`
/// too); without `>` it finds objects (and zones) only.
#[derive(Debug, Default)]
struct Query {
    /// Lowercase; empty = any object.
    object: String,
    /// Lowercase, after `>`.
    part: Option<String>,
}

impl Query {
    fn parse(text: &str) -> Self {
        let text = text.trim().to_lowercase();
        match text.split_once('>') {
            Some((object, part)) => Query {
                object: object.trim().to_owned(),
                part: Some(part.trim().to_owned()),
            },
            None => Query {
                object: text,
                part: None,
            },
        }
    }

    fn is_empty(&self) -> bool {
        self.object.is_empty() && self.part.is_none()
    }

    fn part_matches(&self, part: &str) -> bool {
        self.part
            .as_deref()
            .is_none_or(|query| part.to_lowercase().contains(query))
    }

    /// Whether an object (in `zone`, made of `parts`) shows in the list.
    fn shows(&self, object: &Object, zone: &str, parts: &[String]) -> bool {
        if self.part.is_none() {
            return matches_search(object, &self.object);
        }
        let object_matches = self.object.is_empty()
            || matches_search(object, &self.object)
            || zone.to_lowercase().contains(&self.object);
        object_matches && parts.iter().any(|p| self.part_matches(p))
    }
}

/// Every part a `model > part` search finds, in list order.
fn search_parts(scene: &Scene, parts: &super::ModelParts, query: &Query) -> Vec<super::Pick> {
    let mut found = Vec::new();
    for zone in &scene.zones {
        for (index, object) in scene.objects.iter().enumerate() {
            if object.zone != zone.name {
                continue;
            }
            let names = model_parts(object, parts);
            if !query.shows(object, &zone.name, &names) {
                continue;
            }
            for name in names.into_iter().filter(|p| query.part_matches(p)) {
                found.push(super::Pick {
                    object: index,
                    part: Some(name),
                });
            }
        }
    }
    found
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
    fn model_greater_than_part_finds_that_part_in_every_matching_model() {
        let mut scene = boxes(0);
        let candle = |name: &str, path: &str| crate::scene::Object {
            id: 0,
            hidden: false,
            name: name.into(),
            zone: "test".into(),
            transform: Default::default(),
            components: vec![Component::Model(crate::scene::Model {
                path: path.into(),
                color: glam::Vec3::ONE,
                emissive: glam::Vec3::ZERO,
                collision: true,
                parts: Vec::new(),
            })],
        };
        scene.objects = vec![
            candle("candle", "models/candle.glb"),
            candle("Candle 3", "models/candle.glb"),
            candle("lantern", "models/lantern.glb"),
        ];
        let mut parts = crate::editor::ModelParts::new();
        let names = |list: &[&str]| {
            list.iter()
                .map(|n| (n.to_string(), glam::Vec3::ZERO))
                .collect()
        };
        parts.insert("models/candle.glb".into(), names(&["body", "sphere"]));
        parts.insert("models/lantern.glb".into(), names(&["frame", "sphere"]));

        let found = search_parts(&scene, &parts, &Query::parse("  CANDLE >sphere "));
        let found: Vec<_> = found
            .iter()
            .map(|p| (p.object, p.part.clone().unwrap()))
            .collect();
        assert_eq!(
            found,
            [(0, "sphere".to_string()), (1, "sphere".to_string())]
        );

        // No object query: that part in every model.
        assert_eq!(
            search_parts(&scene, &parts, &Query::parse("> sph")).len(),
            3
        );
        assert!(search_parts(&scene, &parts, &Query::parse("candle > wick")).is_empty());
        // Without `>`, objects only.
        let q = Query::parse("candle");
        assert!(q.part.is_none() && q.shows(&scene.objects[1], "test", &[]));
        assert!(!q.shows(&scene.objects[2], "test", &[]));
    }

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
        editor.undo(&mut scene, &mut PlayerSettings::default());
        assert_eq!(scene, original);

        editor.add_zone(&mut scene);
        assert_eq!(scene.zones.last().unwrap().name, "zone");
        assert!(editor.delete_zone(&mut scene, "zone", None).is_some());
        assert_eq!(scene, original);
    }

    #[test]
    fn deleting_a_zone_moves_or_deletes_its_objects() {
        let mut scene = boxes(3);
        scene.zones.push(crate::scene::Zone::new("hall"));
        scene.objects[1].zone = "hall".into();
        scene.objects[2].components = vec![Component::Portal(Portal {
            to: crate::scene::ZoneRef("hall".into()),
        })];
        let original = scene.clone();
        let mut editor = editor();
        editor.selected = Some(1);

        assert!(
            editor
                .delete_zone(&mut scene, "hall", Some("hall"))
                .is_none()
        );
        assert!(
            editor
                .delete_zone(&mut scene, "hall", Some("nowhere"))
                .is_none()
        );

        // Moved: every object stays, the portal now leads to "test".
        assert!(
            editor
                .delete_zone(&mut scene, "hall", Some("test"))
                .is_some()
        );
        assert_eq!(scene.zones.len(), 1);
        assert_eq!(scene.objects.len(), 3);
        assert!(scene.objects.iter().all(|o| o.zone == "test"));
        assert_eq!(
            scene.objects[2].components[0],
            Component::Portal(Portal {
                to: crate::scene::ZoneRef("test".into())
            })
        );
        assert_eq!(editor.selected, None);
        editor.undo(&mut scene, &mut PlayerSettings::default());
        assert_eq!(scene, original);

        // Deleted with it: only its objects go, even the last zone can go.
        assert!(editor.delete_zone(&mut scene, "hall", None).is_some());
        assert_eq!(scene.objects.len(), 2);
        assert!(editor.delete_zone(&mut scene, "test", None).is_some());
        assert!(scene.zones.is_empty() && scene.objects.is_empty());
        editor.undo(&mut scene, &mut PlayerSettings::default());
        editor.undo(&mut scene, &mut PlayerSettings::default());
        assert_eq!(scene, original);
    }
}

/// An object's row: its name, and its type on the right.
fn object_row(ui: &mut Ui, chosen: bool, object: &Object) -> egui::Response {
    ui.horizontal(|ui| {
        let response = ui.selectable_label(chosen, picked(&object.name, chosen));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(RichText::new(tag(object)).small().color(theme::AMBER_DIM));
        });
        response
    })
    .inner
}

/// What an object mainly is, for its row: CHARACTER, DOOR, LIGHT, BOX...
pub(super) fn tag(object: &Object) -> String {
    let order = |c: &Component| match c {
        Component::PlayerStart => 0,
        Component::Character(_) => 1,
        Component::Pickup(_) => 2,
        Component::SavePoint => 3,
        Component::AbilityGate(_) => 4,
        Component::Switch(_) => 5,
        Component::Door(_) => 6,
        Component::TriggerZone(_) => 7,
        Component::Portal(_) => 8,
        Component::ZoneVolume(_) => 9,
        Component::LightSource(_) => 10,
        Component::SoundSource(_) => 11,
        Component::FogVolume(_) => 12,
        Component::Script(_) => 13,
        Component::Model(_) => 13,
        Component::Primitive(_) => 14,
        _ => 20,
    };
    let Some(main) = object.components.iter().min_by_key(|c| order(c)) else {
        return "EMPTY".into();
    };
    match main {
        Component::PlayerStart => "START".into(),
        Component::Character(_) => "CHARACTER".into(),
        Component::Pickup(_) => "PICKUP".into(),
        Component::SavePoint => "SAVE".into(),
        Component::AbilityGate(_) => "GATE".into(),
        Component::Switch(s) if s.pressure_plate => "PLATE".into(),
        Component::Switch(_) => "SWITCH".into(),
        Component::Door(_) => "DOOR".into(),
        Component::TriggerZone(_) => "TRIGGER".into(),
        Component::Portal(p) => format!("PORTAL › {}", p.to.0.to_uppercase()),
        Component::ZoneVolume(_) => "VOLUME".into(),
        Component::LightSource(_) => "LIGHT".into(),
        Component::SoundSource(_) => "SOUND".into(),
        Component::FogVolume(_) => "FOG".into(),
        Component::Script(s) => {
            format!("SCRIPT › {}", crate::graph::script_name(&s.script.0)).to_uppercase()
        }
        Component::Model(_) => "MODEL".into(),
        Component::Primitive(p) => p.shape.kind().to_uppercase(),
        _ => "OBJECT".into(),
    }
}

/// The outliner's filter chips.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum Filter {
    #[default]
    All,
    Geometry,
    Light,
    Sound,
    Gameplay,
    World,
}

impl Filter {
    const ALL: [Filter; 6] = [
        Filter::All,
        Filter::Geometry,
        Filter::Light,
        Filter::Sound,
        Filter::Gameplay,
        Filter::World,
    ];

    fn title(self) -> &'static str {
        match self {
            Filter::All => "ALL",
            Filter::Geometry => "GEOMETRY",
            Filter::Light => "LIGHT",
            Filter::Sound => "SOUND",
            Filter::Gameplay => "GAMEPLAY",
            Filter::World => "WORLD",
        }
    }

    fn shows(self, object: &Object) -> bool {
        self == Filter::All
            || object.components.iter().any(|c| {
                let kind = match c {
                    Component::Primitive(_) | Component::Model(_) | Component::Material(_) => {
                        Filter::Geometry
                    }
                    Component::LightSource(_) | Component::FogVolume(_) => Filter::Light,
                    Component::SoundSource(_) | Component::AcousticMaterial(_) => Filter::Sound,
                    Component::ZoneVolume(_) | Component::Portal(_) => Filter::World,
                    _ => Filter::Gameplay,
                };
                kind == self
            })
    }
}
