//! The modeler's tools: what the TOOLS panel and its shortcuts do to the
//! open model. Every tool is one undo step.

use super::model_tab::{ModelDoc, SelectMode};
use super::{dim, section};
use crate::modeler::mesh::{Edge, EditMesh};
use egui::{RichText, Ui};
use glam::{Vec2, Vec3};
use std::collections::BTreeSet;
use std::path::Path;

/// The tools' settings, kept per model while it's open.
pub struct ToolSettings {
    /// Meters.
    pub extrude: f32,
    /// Fraction of the way to the face's center.
    pub inset: f32,
    /// Textures per meter for PROJECT UVS.
    pub repeats: f32,
    /// Linear RGB.
    pub color: Vec3,
    /// RANDOMIZE moves each vertex up to this far either way, per axis
    /// (meters).
    pub randomize: Vec3,
}

impl Default for ToolSettings {
    fn default() -> Self {
        Self {
            extrude: 0.25,
            inset: 0.25,
            repeats: 1.0,
            color: Vec3::new(0.8, 0.3, 0.2),
            randomize: Vec3::splat(0.05),
        }
    }
}

/// A tool to run.
#[derive(Debug, Clone, PartialEq)]
pub enum Tool {
    Extrude,
    Inset,
    Delete,
    Merge,
    Randomize,
    Weld,
    Subdivide,
    LoopCut,
    Flip,
    Mirror(usize),
    Smooth(bool),
    Paint,
    ProjectUvs,
    MoveUvs(Vec2),
    ScaleUvs(f32),
    RotateUvs,
    Texture(Option<String>),
}

/// One step when nudging UVs: a sixteenth of the texture.
const UV_STEP: f32 = 1.0 / 16.0;

impl ModelDoc {
    /// Faces the surface tools act on: the selected ones, or (in vertex
    /// and edge mode) those touching the selection; all if nothing is
    /// selected.
    fn target_faces(&self) -> BTreeSet<usize> {
        let faces = match self.mode {
            SelectMode::Face => self.faces.clone(),
            SelectMode::Edge => self.mesh.faces_with_edges(&self.edges),
            SelectMode::Vertex => self.mesh.faces_with_verts(&self.verts),
        };
        if faces.is_empty() && self.selection_count() == 0 {
            (0..self.mesh.faces.len()).collect()
        } else {
            faces
        }
    }

    /// Whether `tool` can run on the current selection.
    pub fn can(&self, tool: &Tool) -> bool {
        let count = self.selection_count();
        match tool {
            Tool::Extrude | Tool::Inset => self.mode == SelectMode::Face && count > 0,
            Tool::Delete | Tool::Randomize => count > 0,
            Tool::Merge => self.mode == SelectMode::Vertex && count > 1,
            Tool::Subdivide => self.mode == SelectMode::Edge && count > 0,
            Tool::LoopCut => self.mode == SelectMode::Edge && count == 1,
            _ => !self.mesh.faces.is_empty(),
        }
    }

    /// Runs `tool` as one undo step. Returns a message for the status bar.
    pub fn apply(&mut self, tool: Tool) -> Option<String> {
        if !self.can(&tool) {
            return None;
        }
        self.finish_drag();
        let before = self.mesh.clone();
        let faces = self.target_faces();
        let verts = self.selected_verts();
        let mut message = None;
        let nothing_selected = self.selection_count() == 0;
        let mesh = &mut self.mesh;
        match tool {
            Tool::Extrude => mesh.extrude(&self.faces, self.tools.extrude),
            Tool::Inset => mesh.inset(&self.faces, self.tools.inset),
            Tool::Delete => {
                let doomed = match self.mode {
                    SelectMode::Face => self.faces.clone(),
                    SelectMode::Edge => mesh.faces_with_edges(&self.edges),
                    SelectMode::Vertex => mesh.faces_with_verts(&self.verts),
                };
                mesh.delete_faces(&doomed);
                self.clear_selection();
            }
            Tool::Merge => {
                let merged = mesh.merge(&self.verts);
                self.clear_selection();
                self.verts.extend(merged);
            }
            Tool::Randomize => {
                let mut random = Random::seeded();
                let most = self.tools.randomize;
                for &v in &verts {
                    let offset = Vec3::new(random.signed(), random.signed(), random.signed());
                    mesh.positions[v as usize] += offset * most;
                }
            }
            Tool::Weld => {
                let count = mesh.weld();
                self.clear_selection();
                message = Some(format!("WELDED {count} CORNERS"));
            }
            Tool::Subdivide => {
                let edges = std::mem::take(&mut self.edges);
                mesh.subdivide(&edges);
            }
            Tool::LoopCut => {
                let start = *self.edges.first().expect("one edge is selected");
                let new = mesh.loop_cut(start);
                // Select the new loop.
                self.edges = loop_edges(mesh, &new);
            }
            Tool::Flip => mesh.flip(&faces),
            Tool::Mirror(axis) => {
                mesh.mirror(axis);
                self.clear_selection();
            }
            Tool::Smooth(smooth) => mesh.set_smooth(&faces, smooth),
            Tool::Paint => {
                let color = self.tools.color;
                match self.mode {
                    _ if nothing_selected => mesh.paint(color, |_, _| true),
                    SelectMode::Face => mesh.paint(color, |f, _| self.faces.contains(&f)),
                    SelectMode::Vertex => mesh.paint(color, |_, v| self.verts.contains(&v)),
                    SelectMode::Edge => {
                        let verts: BTreeSet<u32> = self.edges.iter().flatten().copied().collect();
                        mesh.paint(color, |_, v| verts.contains(&v))
                    }
                }
            }
            Tool::ProjectUvs => mesh.project_uvs(&faces, self.tools.repeats),
            Tool::MoveUvs(by) => mesh.transform_uvs(&faces, |uv, _| uv + by),
            Tool::ScaleUvs(k) => mesh.transform_uvs(&faces, |uv, c| c + (uv - c) * k),
            Tool::RotateUvs => {
                mesh.transform_uvs(&faces, |uv, c| c + (uv - c).perp());
            }
            Tool::Texture(texture) => mesh.texture = texture,
        }
        self.record(before);
        self.after_history();
        message
    }

    /// The TOOLS panel.
    pub fn tools_panel(&mut self, ui: &mut Ui, project_dir: &Path) -> (Option<Tool>, bool) {
        let mut run = None;
        let mut editing = false;

        section(ui, "FACES");
        ui.horizontal(|ui| {
            run = run.take().or(pick(
                ui,
                self,
                "EXTRUDE  E",
                Tool::Extrude,
                "PULL THE SELECTED FACES OUT BY THIS MUCH, WITH SIDE WALLS (E: FOLLOW THE MOUSE)",
            ));
            editing |= ui
                .add(
                    egui::DragValue::new(&mut self.tools.extrude)
                        .speed(0.01)
                        .suffix(" M"),
                )
                .dragged();
        });
        ui.horizontal(|ui| {
            run = run.take().or(pick(
                ui,
                self,
                "INSET  I",
                Tool::Inset,
                "SHRINK EACH SELECTED FACE INSIDE A RING OF NEW FACES",
            ));
            editing |= ui
                .add(
                    egui::DragValue::new(&mut self.tools.inset)
                        .speed(0.01)
                        .range(0.0..=0.99),
                )
                .dragged();
        });
        section(ui, "EDGES");
        ui.horizontal(|ui| {
            run = run.take().or(pick(
                ui,
                self,
                "LOOP CUT  CTRL+R",
                Tool::LoopCut,
                "CUT A NEW EDGE LOOP ACROSS THE SELECTED EDGE",
            ));
            run = run.take().or(pick(
                ui,
                self,
                "SUBDIVIDE",
                Tool::Subdivide,
                "SPLIT THE SELECTED EDGES IN THE MIDDLE",
            ));
        });
        section(ui, "VERTICES");
        ui.horizontal(|ui| {
            run = run.take().or(pick(
                ui,
                self,
                "MERGE  M",
                Tool::Merge,
                "MERGE THE SELECTED VERTICES AT THEIR CENTER",
            ));
            run = run.take().or(pick(
                ui,
                self,
                "WELD",
                Tool::Weld,
                "MERGE VERTICES THAT SIT ON TOP OF EACH OTHER",
            ));
        });
        section(ui, "RANDOMIZE");
        ui.horizontal(|ui| {
            for (axis, value) in ["X", "Y", "Z"].iter().zip(self.tools.randomize.as_mut()) {
                ui.label(dim(axis));
                editing |= ui
                    .add(
                        egui::DragValue::new(value)
                            .speed(0.01)
                            .range(0.0..=100.0)
                            .suffix(" M"),
                    )
                    .on_hover_text("THE MOST A VERTEX MOVES ALONG THIS AXIS, EITHER WAY")
                    .dragged();
            }
        });
        run = run.take().or(pick(
            ui,
            self,
            "RANDOMIZE",
            Tool::Randomize,
            "MOVE EACH SELECTED VERTEX BY A RANDOM AMOUNT, UP TO THE DISTANCES ABOVE EITHER WAY",
        ));
        section(ui, "MESH");
        ui.horizontal(|ui| {
            run = run.take().or(pick(
                ui,
                self,
                "DELETE  X",
                Tool::Delete,
                "DELETE THE SELECTION (AND THE FACES THAT USE IT)",
            ));
            run = run.take().or(pick(
                ui,
                self,
                "FLIP",
                Tool::Flip,
                "TURN THE FACES TO FACE THE OTHER WAY",
            ));
        });
        ui.horizontal(|ui| {
            ui.label(dim("MIRROR"));
            for (axis, label) in ["X", "Y", "Z"].iter().enumerate() {
                run = run.take().or(pick(
                    ui,
                    self,
                    label,
                    Tool::Mirror(axis),
                    "ADD A MIRROR IMAGE ACROSS THE CENTER, JOINED WHERE IT TOUCHES",
                ));
            }
        });

        section(ui, "SHADING");
        ui.horizontal(|ui| {
            run = run.take().or(pick(
                ui,
                self,
                "FLAT",
                Tool::Smooth(false),
                "ONE NORMAL PER FACE: CRISP FACETS",
            ));
            run = run.take().or(pick(
                ui,
                self,
                "SMOOTH",
                Tool::Smooth(true),
                "NORMALS BLEND ACROSS NEIGHBOURING SMOOTH FACES",
            ));
        });
        section(ui, "COLOR");
        ui.horizontal(|ui| {
            let mut rgb = self.tools.color.to_array();
            if egui::color_picker::color_edit_button_rgb(ui, &mut rgb).changed() {
                self.tools.color = Vec3::from_array(rgb);
            }
            run = run.take().or(pick(
                ui,
                self,
                "PAINT",
                Tool::Paint,
                "PAINT THE SELECTION (OR EVERYTHING) WITH THIS COLOR",
            ));
        });

        section(ui, "MATERIAL");
        self.surface_sliders(ui);
        let current = self.mesh.texture.clone();
        let label = current
            .clone()
            .unwrap_or_else(|| "NONE".into())
            .to_uppercase();
        ui.label(dim("TEXTURE"));
        egui::ComboBox::from_id_salt("model_texture")
            .selected_text(label)
            .show_ui(ui, |ui| {
                if ui.selectable_label(current.is_none(), "NONE").clicked() && current.is_some() {
                    run = Some(Tool::Texture(None));
                }
                for path in texture_files(project_dir) {
                    let chosen = current.as_deref() == Some(path.as_str());
                    if ui.selectable_label(chosen, path.to_uppercase()).clicked() && !chosen {
                        run = Some(Tool::Texture(Some(path)));
                    }
                }
            });
        ui.horizontal(|ui| {
            run = run.take().or(pick(
                ui,
                self,
                "PROJECT UVS",
                Tool::ProjectUvs,
                "MAP THE TEXTURE ONTO THE FACES ALONG THEIR MAIN AXIS",
            ));
            editing |= ui
                .add(
                    egui::DragValue::new(&mut self.tools.repeats)
                        .speed(0.05)
                        .range(0.01..=64.0)
                        .suffix(" / M"),
                )
                .on_hover_text("TEXTURE REPEATS PER METER")
                .dragged();
        });
        ui.horizontal(|ui| {
            let s = UV_STEP;
            for (label, by) in [
                ("←", Vec2::new(-s, 0.0)),
                ("→", Vec2::new(s, 0.0)),
                ("↑", Vec2::new(0.0, -s)),
                ("↓", Vec2::new(0.0, s)),
            ] {
                run = run.take().or(pick(
                    ui,
                    self,
                    label,
                    Tool::MoveUvs(by),
                    "MOVE THE TEXTURE ON THE FACES BY 1/16",
                ));
            }
            run = run.take().or(pick(
                ui,
                self,
                "×2",
                Tool::ScaleUvs(0.5),
                "TEXTURE TWICE AS BIG",
            ));
            run = run.take().or(pick(
                ui,
                self,
                "÷2",
                Tool::ScaleUvs(2.0),
                "TEXTURE HALF AS BIG",
            ));
            run = run
                .take()
                .or(pick(ui, self, "⟳", Tool::RotateUvs, "TURN THE TEXTURE 90°"));
        });
        ui.label(
            RichText::new(
                "SURFACE TOOLS ACT ON THE SELECTION, OR ON EVERYTHING WHEN NOTHING IS SELECTED",
            )
            .small()
            .color(crate::ui::theme::AMBER_DIM),
        );
        (run, editing)
    }
}

impl ModelDoc {
    /// ROUGHNESS and METALNESS: the model changes as a slider moves; one
    /// undo step when it's let go (or a value is typed).
    fn surface_sliders(&mut self, ui: &mut Ui) {
        let mut dragging = false;
        for (label, hint, metal) in [
            ("ROUGHNESS", "0 = MIRROR-SMOOTH, 1 = FULLY MATTE", false),
            (
                "METALNESS",
                "0 = STONE, WOOD, PLASTER; 1 = BARE METAL",
                true,
            ),
        ] {
            ui.horizontal(|ui| {
                ui.label(dim(label));
                let surface = self.mesh.surface;
                let mut value = if metal {
                    surface.metalness
                } else {
                    surface.roughness
                };
                let response = ui
                    .add(egui::Slider::new(&mut value, 0.0..=1.0))
                    .on_hover_text(hint);
                if response.changed() {
                    // Before the first change: what undo goes back to.
                    self.surface_before.get_or_insert_with(|| self.mesh.clone());
                    let field = if metal {
                        &mut self.mesh.surface.metalness
                    } else {
                        &mut self.mesh.surface.roughness
                    };
                    *field = value;
                    self.changed = true;
                }
                dragging |= response.dragged();
            });
        }
        if !dragging && let Some(before) = self.surface_before.take() {
            self.record(before);
        }
    }
}

/// Random numbers for RANDOMIZE (splitmix64), seeded from the clock.
struct Random(u64);

impl Random {
    fn seeded() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64);
        Self(nanos)
    }

    /// Evenly spread in -1..1.
    fn signed(&mut self) -> f32 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        // The top 24 bits as 0..1, then stretched to -1..1.
        (z >> 40) as f32 / (1u64 << 24) as f32 * 2.0 - 1.0
    }
}

/// A tool button, greyed out when the tool can't run; the tool if clicked.
fn pick(ui: &mut Ui, doc: &ModelDoc, label: &str, tool: Tool, hint: &str) -> Option<Tool> {
    ui.add_enabled(doc.can(&tool), egui::Button::new(label))
        .on_hover_text(hint)
        .clicked()
        .then_some(tool)
}

/// Edges whose both ends are in `verts` (a new loop).
fn loop_edges(mesh: &EditMesh, verts: &BTreeSet<u32>) -> BTreeSet<Edge> {
    mesh.edges()
        .into_iter()
        .filter(|e| verts.contains(&e[0]) && verts.contains(&e[1]))
        .collect()
}

/// Images in the project's `textures/` folder, relative to the project.
fn texture_files(project_dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(project_dir.join("textures")) else {
        return Vec::new();
    };
    let mut files: Vec<String> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension().is_some_and(|e| {
                let e = e.to_string_lossy().to_ascii_lowercase();
                e == "png" || e == "jpg" || e == "jpeg"
            })
        })
        .filter_map(|p| {
            p.strip_prefix(project_dir)
                .ok()
                .map(|r| r.to_string_lossy().replace('\\', "/"))
        })
        .collect();
    files.sort();
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_need_the_right_selection_and_undo_in_one_step() {
        let mut doc = ModelDoc::new(EditMesh::cube(Vec3::ONE), "test".into());
        assert!(!doc.can(&Tool::Extrude), "nothing selected");
        doc.faces.insert(3);
        assert!(doc.can(&Tool::Extrude));
        assert!(!doc.can(&Tool::LoopCut), "face mode");
        doc.apply(Tool::Extrude);
        assert_eq!(doc.mesh.faces.len(), 10);
        assert_eq!(doc.selection_count(), 1, "the extruded face stays selected");
        doc.undo();
        assert_eq!(doc.mesh.faces.len(), 6);

        doc.clear_selection();
        doc.mode = SelectMode::Edge;
        doc.edges.insert(crate::modeler::mesh::edge(0, 2));
        doc.apply(Tool::LoopCut);
        assert_eq!(doc.mesh.faces.len(), 10);
        assert_eq!(doc.selection_count(), 4, "the new loop is selected");

        // Randomize stays within the distances, and moves shared corners
        // together (faces stay joined).
        doc.clear_selection();
        doc.mode = SelectMode::Vertex;
        doc.select_all();
        doc.tools.randomize = Vec3::new(0.0, 0.5, 0.0);
        let before = doc.mesh.clone();
        doc.apply(Tool::Randomize);
        let mut moved = false;
        for (a, b) in doc.mesh.positions.iter().zip(&before.positions) {
            assert_eq!((a.x, a.z), (b.x, b.z), "X and Z are 0: untouched");
            assert!((a.y - b.y).abs() <= 0.5 + 1e-6);
            moved |= a.y != b.y;
        }
        assert!(moved);
        assert_eq!(doc.mesh.faces, before.faces);
        doc.undo();
        assert_eq!(doc.mesh, before, "one undo step");

        // With nothing selected, surface tools act on everything.
        doc.clear_selection();
        doc.apply(Tool::Paint);
        assert!(
            doc.mesh
                .faces
                .iter()
                .all(|f| f.colors.len() == f.verts.len())
        );
    }
}
