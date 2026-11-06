//! The inspector for the selected object: name, zone, transform and its
//! components.

use super::history::Change;
use super::player_panel::player_inspector;
use super::{Editor, EditorOutput, dim, inspector, section};
use crate::player::settings::PlayerSettings;
use crate::render::mesh::Shape;
use crate::scene::{
    AcousticMaterial, Component, Door, Floating, FogVolume, LightSource, Material, PhysicsBody,
    Portal, Scene, SoundSource, Switch, ZoneVolume,
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
        ui.add_space(4.0);
        ui.label(RichText::new("INSPECTOR").strong());
        ui.add_space(4.0);
        let Some(index) = self.selected.filter(|&i| i < scene.objects.len()) else {
            if !self.settings_inspector(ui, scene, output) {
                ui.label(dim("NOTHING SELECTED"));
            }
            return;
        };

        ui.add_space(4.0);

        // For reference pickers.
        let mut objects: Vec<String> = scene.objects.iter().map(|o| o.name.clone()).collect();
        objects.sort();
        let names = inspector::Names {
            objects,
            zones: scene.zones.iter().map(|z| z.name.clone()).collect(),
        };
        let zones = &scene.zones;
        let object = &mut scene.objects[index];
        let before = object.clone();
        let player_before = player.clone();
        let project_dir = self.project_dir.as_path();

        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    inspector::field_label(ui, "NAME");
                    ui.text_edit_singleline(&mut object.name);
                });
                ui.horizontal(|ui| {
                    inspector::field_label(ui, "ZONE");
                    egui::ComboBox::from_id_salt("inspector_zone")
                        .selected_text(object.zone.to_uppercase())
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

                section(ui, "TRANSFORM");
                inspector::fields(ui, &mut object.transform, &names);

                let mut remove = None;
                for (i, component) in object.components.iter_mut().enumerate() {
                    if component_header(ui, &inspector::title(component)) {
                        remove = Some(i);
                    }
                    if let Component::Primitive(primitive) = component {
                        shape_kind(ui, i, &mut primitive.shape);
                    }
                    inspector::fields(ui, component, &names);
                }
                if let Some(i) = remove {
                    object.components.remove(i);
                }
                if object.components.contains(&Component::PlayerStart) {
                    player_inspector(ui, player, project_dir);
                }

                ui.add_space(10.0);
                ui.menu_button("ADD COMPONENT", |ui| {
                    for (name, component) in new_components() {
                        let kind = std::mem::discriminant(&component);
                        let present = object
                            .components
                            .iter()
                            .any(|c| std::mem::discriminant(c) == kind);
                        if ui
                            .add_enabled(!present, egui::Button::new(name.to_uppercase()))
                            .clicked()
                        {
                            object.components.push(component);
                        }
                    }
                });
            });

        if *object != before {
            self.begin_edit(index, &before);
            output.changed.push(Change::Modified(index));
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
pub(super) fn new_components() -> [(&'static str, Component); 11] {
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
    ]
}

/// A component's title with a REMOVE button; returns true if it was clicked.
pub(super) fn component_header(ui: &mut Ui, title: &str) -> bool {
    ui.add_space(10.0);
    let removed = ui
        .horizontal(|ui| {
            ui.label(RichText::new(title).strong());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.small_button("REMOVE").clicked()
            })
            .inner
        })
        .inner;
    ui.separator();
    removed
}
