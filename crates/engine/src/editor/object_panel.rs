//! The inspector for the selected object: name, zone, transform and its
//! components.

use super::history::Change;
use super::player_panel::player_inspector;
use super::selection::own_settings;
use super::settings_panel::own_toggle;
use super::{Editor, EditorOutput, dim, inspector, section};
use crate::player::settings::PlayerSettings;
use crate::render::mesh::Shape;
use crate::scene::{
    AcousticMaterial, Character, Component, Door, Floating, FogVolume, LightSource, Material,
    Object, ObjectRef, PhysicsBody, Pickup, Portal, Scene, SoundSource, Switch, TriggerZone,
    ZoneVolume,
};
use egui::{Align, Layout, RichText, Ui};

impl Editor {
    pub(super) fn inspector(
        &mut self,
        ui: &mut Ui,
        scene: &mut Scene,
        player: &mut PlayerSettings,
        output: &mut EditorOutput,
    ) {
        let Some(index) = self.selected.filter(|&i| i < scene.objects.len()) else {
            if !self.settings_inspector(ui, scene, output) {
                ui.label(dim("NOTHING SELECTED"));
            }
            return;
        };

        ui.add_space(4.0);

        // For reference pickers.
        self.index.refresh(&self.project_dir, false);
        let names = self.index.for_scene(scene);
        let zones = &scene.zones;
        let object = &mut scene.objects[index];
        let before = object.clone();
        let several = self.edits_several();
        if several {
            ui.label(dim(&format!(
                "{} SELECTED: CHANGES GO TO EVERY SELECTED {}",
                self.picks().len(),
                if self.selected_part.is_some() {
                    "PART"
                } else {
                    "OBJECT"
                }
            )));
        }
        if let Some(part) = self.selected_part.clone() {
            if part_inspector(ui, object, &part, &names) {
                self.selected_part = None;
            }
            if *object != before {
                if several {
                    self.spread_part_edit(scene, index, &before, output);
                } else {
                    self.begin_edit(index, &before);
                    output.changed.push(Change::Modified(index));
                }
            }
            return;
        }
        let player_before = player.clone();
        let project_dir = self.project_dir.as_path();
        let scene_key = crate::graph::key(&self.scene_path);
        let character = object.components.iter().find_map(|c| match c {
            Component::Character(ch) if !ch.id.is_empty() => Some(ch.id.clone()),
            _ => None,
        });
        let mentions =
            self.index
                .mentions(&scene_key, &ObjectRef::to(object), character.as_deref());
        let mut open_graph = None;

        ui.horizontal(|ui| {
            let name = ui
                .add(
                    egui::TextEdit::singleline(&mut object.name)
                        .desired_width((ui.available_width() - 130.0).max(80.0))
                        .font(egui::TextStyle::Heading),
                )
                .on_hover_text(
                    "NAME: UNIQUE IN THE SCENE. RENAMING KEEPS GRAPHS AND SWITCHES LINKED",
                );
            if std::mem::take(&mut self.focus_name) {
                name.request_focus();
            }
            // Names stay unique: a taken one gets a number. (`names` is
            // from this frame's start, with this object's name in it once.)
            if name.lost_focus() && names.objects.iter().filter(|n| **n == object.name).count() > 1
            {
                let mut others = names.objects.clone();
                if let Some(own) = others.iter().position(|n| *n == object.name) {
                    others.remove(own);
                }
                object.name =
                    super::objects::unique_among(others.iter().map(String::as_str), &object.name);
            }
            egui::ComboBox::from_id_salt("inspector_zone")
                .selected_text(format!("ZONE: {}", object.zone.to_uppercase()))
                .width(120.0)
                .show_ui(ui, |ui| {
                    for zone in zones {
                        ui.selectable_value(
                            &mut object.zone,
                            zone.name.clone(),
                            zone.name.to_uppercase(),
                        );
                    }
                });
        });
        ui.add_space(4.0);

        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                let t = object.transform;
                let summary = format!(
                    "{:.1} {:.1} {:.1} · {:.0}°",
                    t.position.x, t.position.y, t.position.z, t.rotation.y
                );
                card(
                    ui,
                    "card_transform",
                    "TRANSFORM",
                    &summary,
                    false,
                    false,
                    |ui| {
                        inspector::fields(ui, &mut object.transform, &names);
                    },
                );

                // One owner per transform: a script fights a door or a
                // physics body (Floating adds its bob to the script's moves).
                let has = |f: fn(&Component) -> bool| object.components.iter().any(f);
                if has(|c| matches!(c, Component::Script(_)))
                    && has(|c| matches!(c, Component::Door(_) | Component::PhysicsBody(_)))
                {
                    ui.label(
                        RichText::new(
                            "A SCRIPT AND A DOOR OR PHYSICS BODY BOTH MOVE THIS OBJECT:                              THE SCRIPT WINS EACH FRAME. KEEP ONE",
                        )
                        .color(crate::ui::theme::ALERT),
                    );
                }
                let mut remove = None;
                for (i, component) in object.components.iter_mut().enumerate() {
                    let title = inspector::title(component);
                    let group = group_of(component);
                    let id = format!("card_{i}_{title}");
                    if card(ui, &id, &title, group, true, true, |ui| {
                        if let Component::Primitive(primitive) = component {
                            shape_kind(ui, i, &mut primitive.shape);
                        }
                        inspector::fields(ui, component, &names);
                    }) {
                        remove = Some(i);
                    }
                }
                if let Some(i) = remove {
                    object.components.remove(i);
                }
                if object.components.contains(&Component::PlayerStart) {
                    card(
                        ui,
                        "card_player",
                        "PLAYER FEEL",
                        "PLAYER.RON",
                        false,
                        false,
                        |ui| {
                            player_inspector(ui, player, project_dir);
                        },
                    );
                }

                let count = format!("{} MENTIONS", mentions.len());
                card(ui, "card_logic", "IN LOGIC", &count, true, false, |ui| {
                    if mentions.is_empty() {
                        ui.label(dim("NO GRAPH NAMES THIS OBJECT YET"));
                    }
                    for (graph, what) in &mentions {
                        ui.horizontal(|ui| {
                            ui.label(graph.trim_start_matches("graphs/"));
                            ui.label(dim(what));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.small_button("OPEN").clicked() {
                                    open_graph = Some(std::path::PathBuf::from(graph));
                                }
                            });
                        });
                    }
                });

                ui.add_space(10.0);
                add_behavior_menu(ui, object);
            });
        if let Some(path) = open_graph {
            output.open_graph = Some(path);
        }

        if *object != before {
            if several {
                self.spread_object_edit(scene, index, &before, output);
            } else {
                self.begin_edit(index, &before);
                output.changed.push(Change::Modified(index));
            }
        }
        if *player != player_before {
            self.begin_player_edit(player_before, player);
            output.changed.push(Change::Player);
        }
    }
}

/// Picks a primitive's shape type; the new shape keeps about the same size.
pub(super) fn shape_kind(ui: &mut Ui, id: usize, shape: &mut Shape) {
    ui.horizontal(|ui| {
        inspector::field_label(ui, "TYPE");
        egui::ComboBox::from_id_salt(("shape_kind", id))
            .selected_text(shape.kind().to_uppercase())
            .show_ui(ui, |ui| {
                for kind in Shape::KINDS {
                    if ui
                        .selectable_label(shape.kind() == kind, kind.to_uppercase())
                        .clicked()
                        && shape.kind() != kind
                    {
                        *shape = shape.with_kind(kind);
                    }
                }
            });
    });
}

/// Components the inspector can add (one of each per object).
pub(super) fn new_components() -> [(&'static str, Component); 17] {
    [
        ("floating", Component::Floating(Floating::default())),
        ("door", Component::Door(Door::default())),
        ("switch", Component::Switch(Switch::default())),
        (
            "physics body",
            Component::PhysicsBody(PhysicsBody::default()),
        ),
        (
            "sound source",
            Component::SoundSource(SoundSource::default()),
        ),
        (
            "acoustic material",
            Component::AcousticMaterial(AcousticMaterial::default()),
        ),
        ("zone volume", Component::ZoneVolume(ZoneVolume::default())),
        ("portal", Component::Portal(Portal::default())),
        (
            "light source",
            Component::LightSource(LightSource::default()),
        ),
        ("fog volume", Component::FogVolume(FogVolume::default())),
        ("material", Component::Material(Material::default())),
        (
            "trigger zone",
            Component::TriggerZone(TriggerZone::default()),
        ),
        ("character", Component::Character(Character::default())),
        ("pickup", Component::Pickup(Pickup::default())),
        ("save point", Component::SavePoint),
        (
            "visible when",
            Component::VisibleWhen(crate::scene::VisibleWhen::default()),
        ),
        ("script", Component::Script(crate::scene::Script::default())),
    ]
}

/// A collapsible inspector card: title, a short note on the right and (if
/// `removable`) REMOVE. Returns true if REMOVE was clicked.
fn card(
    ui: &mut Ui,
    id: &str,
    title: &str,
    note: &str,
    open: bool,
    removable: bool,
    body: impl FnOnce(&mut Ui),
) -> bool {
    let id = ui.make_persistent_id(id);
    let mut removed = false;
    ui.add_space(2.0);
    egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, open)
        .show_header(ui, |ui| {
            ui.label(RichText::new(title).strong());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if removable {
                    removed = ui.small_button("REMOVE").clicked();
                }
                ui.label(
                    RichText::new(note)
                        .small()
                        .color(crate::ui::theme::AMBER_DIM),
                );
            });
        })
        .body(|ui| {
            body(ui);
            ui.add_space(4.0);
        });
    ui.separator();
    removed
}

/// Which ADD BEHAVIOR group a component is in.
fn group_of(component: &Component) -> &'static str {
    match component {
        Component::Primitive(_) | Component::Model(_) => "SHAPE",
        Component::Material(_) | Component::LightSource(_) | Component::FogVolume(_) => "LOOK",
        Component::Door(_) | Component::Floating(_) | Component::PhysicsBody(_) => "MOTION",
        Component::SoundSource(_) | Component::AcousticMaterial(_) => "SOUND",
        Component::ZoneVolume(_) | Component::Portal(_) => "WORLD",
        _ => "GAMEPLAY",
    }
}

/// `new_components()` by group, with a line each: (group, [(index, line)]).
pub(super) const BEHAVIOR_GROUPS: [(&str, &[(usize, &str)]); 5] = [
    (
        "GAMEPLAY",
        &[
            (2, "E or stand on it to flip a door"),
            (11, "a box graphs wait for"),
            (12, "talk with E; runs a dialogue"),
            (13, "E to take; sets a flag"),
            (14, "E saves the game here"),
            (15, "blocks the way until a flag"),
            (16, "hidden until a graph shows it"),
            (17, "runs a script of yours on it"),
        ],
    ),
    (
        "MOTION",
        &[
            (1, "swings or slides open"),
            (0, "bobs up and down"),
            (3, "falls, tumbles, can be pushed"),
        ],
    ),
    (
        "SOUND",
        &[
            (4, "plays through the acoustics"),
            (5, "how the surface sounds"),
        ],
    ),
    (
        "LOOK",
        &[
            (10, "texture, roughness, metal"),
            (8, "lantern, candle, brazier"),
            (9, "local mist or smoke"),
        ],
    ),
    (
        "WORLD",
        &[(6, "where a zone is"), (7, "doorway between zones")],
    ),
];

/// The ADD BEHAVIOR menu: every component by purpose, with a line each;
/// those the object has already are greyed (one of each per object).
fn add_behavior_menu(ui: &mut Ui, object: &mut Object) {
    let components = new_components();
    ui.menu_button(RichText::new("+ ADD BEHAVIOR").strong(), |ui| {
        ui.set_min_width(560.0);
        egui::Grid::new("add_behavior")
            .num_columns(2)
            .spacing([20.0, 6.0])
            .show(ui, |ui| {
                for row in BEHAVIOR_GROUPS.chunks(2) {
                    for (group, entries) in row {
                        ui.vertical(|ui| {
                            ui.label(
                                RichText::new(*group)
                                    .small()
                                    .color(crate::ui::theme::AMBER_DIM),
                            );
                            for (index, line) in entries.iter() {
                                let (name, component) = &components[*index];
                                let kind = std::mem::discriminant(component);
                                let present = object
                                    .components
                                    .iter()
                                    .any(|c| std::mem::discriminant(c) == kind);
                                let button =
                                    egui::Button::new(format!("{}\n{}", name.to_uppercase(), line))
                                        .wrap_mode(egui::TextWrapMode::Extend);
                                if ui
                                    .add_enabled(!present, button)
                                    .on_disabled_hover_text("ALREADY ADDED")
                                    .clicked()
                                {
                                    object.components.push(component.clone());
                                    ui.close();
                                }
                            }
                        });
                    }
                    ui.end_row();
                }
            });
    });
}

/// The settings of one part of the object's model. Returns true if BACK TO
/// THE OBJECT was clicked.
fn part_inspector(ui: &mut Ui, object: &mut Object, part: &str, names: &inspector::Names) -> bool {
    let Some(model) = object.components.iter_mut().find_map(|c| match c {
        Component::Model(m) => Some(m),
        _ => None,
    }) else {
        return true;
    };
    let mut back = false;
    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                inspector::field_label(ui, "PART");
                ui.label(part.to_uppercase());
            });
            ui.horizontal(|ui| {
                inspector::field_label(ui, "OF");
                ui.label(object.name.to_uppercase());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    back = ui.small_button("BACK TO THE OBJECT").clicked();
                });
            });
            section(ui, "SETTINGS");
            let own = model.parts.iter().position(|p| p.name == part);
            let Some(own) = own else {
                ui.label(dim("USES THE MODEL'S COLOR, GLOW AND COLLISION"));
                if ui.small_button("GIVE IT ITS OWN").clicked() {
                    own_settings(model, part);
                }
                return;
            };
            let settings = &mut model.parts[own];
            inspector::fields_except(ui, settings, names, &["name", "light"]);
            section(ui, "LIGHT");
            ui.label(dim("SHINES FROM THE PART'S CENTER"));
            own_toggle(ui, "LIGHT", &mut settings.light, &LightSource::default());
            if let Some(light) = &mut settings.light {
                inspector::fields(ui, light, names);
            }
            ui.add_space(10.0);
            if ui
                .small_button("USE THE MODEL'S SETTINGS")
                .on_hover_text("DROP THIS PART'S OWN SETTINGS AND LIGHT")
                .clicked()
            {
                model.parts.remove(own);
            }
        });
    back
}
