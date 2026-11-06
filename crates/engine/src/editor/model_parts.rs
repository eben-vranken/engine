//! The modeler's PARTS: a model is made of named parts (a candle's stick,
//! wax and flame), so scenes can give each its own color, glow, collision
//! or light. Here parts are added from a primitive, picked, renamed,
//! deleted, and faces moved between them.

use super::model_tab::{ModelDoc, SelectMode};
use super::name_dialog::{STARTS, start_mesh};
use super::{dim, section};
use crate::modeler::mesh::EditMesh;
use egui::{Align, Layout, RichText, Ui};
use glam::Vec3;

impl ModelDoc {
    /// The PARTS section of the left panel.
    pub(super) fn parts_panel(&mut self, ui: &mut Ui) {
        section(ui, "PARTS");
        let names = self.mesh.part_names();
        let current = self.current_part();
        let mut pick = None;
        let mut delete = None;
        for (i, name) in names.iter().enumerate() {
            let i = i as u32;
            ui.horizontal(|ui| {
                let chosen = current == Some(i);
                let text = if chosen {
                    RichText::new(name.to_uppercase()).strong()
                } else {
                    RichText::new(name.to_uppercase())
                };
                if ui
                    .selectable_label(chosen, text)
                    .on_hover_text("SELECT ITS FACES")
                    .clicked()
                {
                    pick = Some(i);
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .add_enabled(names.len() > 1, egui::Button::new("X").small())
                        .on_hover_text("DELETE THIS PART AND ITS FACES")
                        .on_disabled_hover_text("A MODEL KEEPS AT LEAST ONE PART")
                        .clicked()
                    {
                        delete = Some(i);
                    }
                });
            });
        }
        if let Some(part) = pick {
            self.select_part(part);
        }
        if let Some(part) = delete {
            let before = self.mesh.clone();
            if self.mesh.remove_part(part) {
                self.clear_selection();
                self.record(before);
                self.after_history();
            }
        }

        // Rename the part whose faces are selected.
        if let Some(part) = self.current_part() {
            let name = &mut self.part_name;
            if name.as_ref().is_none_or(|(p, _)| *p != part) {
                *name = Some((part, names[part as usize].clone()));
            }
            let mut rename = None;
            if let Some((_, typed)) = name {
                ui.horizontal(|ui| {
                    ui.label(dim("NAME"));
                    let field = ui.text_edit_singleline(typed);
                    if field.lost_focus() {
                        rename = Some(typed.trim().to_owned());
                    }
                });
            }
            if let Some(new) = rename {
                self.rename_part(part, new);
            }
        } else {
            self.part_name = None;
        }

        ui.horizontal(|ui| {
            ui.menu_button("+ PART", |ui| {
                ui.label(dim("A NEW PART ON TOP OF THE MODEL"));
                for (i, start) in STARTS.iter().enumerate() {
                    if ui.button(*start).clicked() {
                        self.add_part(&start.to_lowercase(), start_mesh(i));
                        ui.close();
                    }
                }
            })
            .response
            .on_hover_text("ADD A BOX, PLANE, CYLINDER OR SPHERE AS A NEW PART");
            let has_faces = self.mode == SelectMode::Face && !self.faces.is_empty();
            ui.add_enabled_ui(has_faces, |ui| {
                ui.menu_button("MOVE TO", |ui| {
                    for (i, name) in names.iter().enumerate() {
                        if ui.button(name.to_uppercase()).clicked() {
                            self.move_faces_to(Some(i as u32));
                            ui.close();
                        }
                    }
                    ui.separator();
                    if ui.button("A NEW PART").clicked() {
                        self.move_faces_to(None);
                        ui.close();
                    }
                })
                .response
                .on_hover_text("PUT THE SELECTED FACES IN ANOTHER PART")
                .on_disabled_hover_text("SELECT FACES FIRST (FACE MODE)");
            });
        });
        ui.label(dim("SCENES GIVE EACH PART ITS OWN COLOR, GLOW OR LIGHT"));
    }

    /// The part all selected faces are in, if they share one.
    fn current_part(&self) -> Option<u32> {
        if self.mode != SelectMode::Face {
            return None;
        }
        let mut parts = self.faces.iter().map(|&f| self.mesh.part_of(f));
        let first = parts.next()?;
        parts.all(|p| p == first).then_some(first)
    }

    /// Selects every face of `part` (in face mode).
    fn select_part(&mut self, part: u32) {
        self.set_mode(SelectMode::Face);
        self.clear_selection();
        self.faces = (0..self.mesh.faces.len())
            .filter(|&f| self.mesh.part_of(f) == part)
            .collect();
    }

    /// Adds `mesh` as a new part standing on top of the model, and selects
    /// it so the gizmo moves it.
    fn add_part(&mut self, kind: &str, mut mesh: EditMesh) {
        let top = self
            .mesh
            .positions
            .iter()
            .map(|p| p.y)
            .fold(f32::NEG_INFINITY, f32::max);
        let bottom = mesh
            .positions
            .iter()
            .map(|p| p.y)
            .fold(f32::INFINITY, f32::min);
        if top.is_finite() && bottom.is_finite() {
            let lift = Vec3::Y * (top - bottom);
            mesh.positions.iter_mut().for_each(|p| *p += lift);
        }
        let before = self.mesh.clone();
        let name = self.mesh.free_part_name(kind);
        let part = self.mesh.add_part(name, mesh);
        self.record(before);
        self.after_history();
        self.select_part(part);
    }

    fn rename_part(&mut self, part: u32, name: String) {
        let names = self.mesh.part_names();
        if name.is_empty() || names.contains(&name) {
            self.part_name = None; // back to the old name
            return;
        }
        let before = self.mesh.clone();
        self.mesh.parts = names;
        self.mesh.parts[part as usize] = name;
        self.record(before);
    }

    /// Moves the selected faces to `part`, or to a new part.
    fn move_faces_to(&mut self, part: Option<u32>) {
        let before = self.mesh.clone();
        let part = part.unwrap_or_else(|| {
            let name = self.mesh.free_part_name("part");
            self.mesh.add_part(name, EditMesh::default())
        });
        if self.mesh.parts.is_empty() {
            self.mesh.parts = self.mesh.part_names();
        }
        for &f in &self.faces {
            self.mesh.faces[f].part = part;
        }
        // A part left without faces goes away.
        let used: std::collections::BTreeSet<u32> = (0..self.mesh.faces.len())
            .map(|f| self.mesh.part_of(f))
            .collect();
        let mut part = part;
        for empty in (0..self.mesh.parts.len() as u32).rev() {
            if !used.contains(&empty) && self.mesh.parts.len() > 1 {
                for face in &mut self.mesh.faces {
                    if face.part > empty {
                        face.part -= 1;
                    }
                }
                self.mesh.parts.remove(empty as usize);
                if part > empty {
                    part -= 1;
                }
            }
        }
        self.record(before);
        self.after_history();
        self.select_part(part);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parts_are_added_on_top_moved_and_deleted() {
        let mut doc = ModelDoc::new(EditMesh::cube(Vec3::ONE), "test".into());
        doc.add_part("cylinder", start_mesh(2));
        assert_eq!(doc.mesh.part_names(), ["body", "cylinder"]);
        // Standing on the box (top at 0.5).
        let lowest = doc
            .mesh
            .faces
            .iter()
            .filter(|f| f.part == 1)
            .flat_map(|f| &f.verts)
            .map(|&v| doc.mesh.positions[v as usize].y)
            .fold(f32::INFINITY, f32::min);
        assert!((lowest - 0.5).abs() < 1e-5);
        assert_eq!(doc.current_part(), Some(1));

        // Two of the box's faces move to a new part.
        doc.select_part(0);
        doc.faces = [0, 1].into();
        doc.move_faces_to(None);
        assert_eq!(doc.mesh.part_names().len(), 3);
        assert_eq!(doc.current_part(), Some(2));

        doc.rename_part(2, "flame".into());
        assert_eq!(doc.mesh.part_names()[2], "flame");
        doc.rename_part(2, "body".into()); // taken
        assert_eq!(doc.mesh.part_names()[2], "flame");

        let faces = doc.mesh.faces.len();
        assert!(doc.mesh.remove_part(1));
        assert_eq!(doc.mesh.part_names(), ["body", "flame"]);
        assert!(doc.mesh.faces.len() < faces);
        assert!(doc.mesh.faces.iter().all(|f| f.part < 2));

        // Undo goes back step by step.
        doc.undo();
        assert_eq!(doc.mesh.part_names(), ["body", "cylinder", "part"]);
    }

    #[test]
    fn each_part_is_its_own_range_and_survives_saving() {
        let mut mesh = EditMesh::cube(Vec3::ONE);
        mesh.add_part("flame".into(), start_mesh(3));
        let model = mesh.to_model(None);
        let names: Vec<_> = model.parts.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["body", "flame"]);
        assert!(
            model.part_center("flame").unwrap().length() < 1e-4,
            "the sphere is centered"
        );

        let dir = std::env::temp_dir().join(format!("engine-parts-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("candle.glb");
        crate::modeler::glb::save(&file, &mesh, &dir).unwrap();
        // The engine loads it with the part names; the modeler gets its mesh back.
        let loaded = crate::assets::load_gltf(&file).unwrap();
        assert_eq!(loaded.part_names(), ["body", "flame"]);
        assert_eq!(crate::modeler::glb::load(&file).unwrap(), mesh);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
