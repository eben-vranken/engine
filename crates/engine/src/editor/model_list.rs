//! The MODELS list under the scene panel: every model in the project, with
//! a search. Drag one into the viewport to place it there, `+` to place it
//! in front of the camera.

use super::files::all_models;
use super::{Editor, EditorOutput, dim};
use egui::{Align, Layout, Pos2, Ui};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// How often the list looks at the folder again (new saves and imports
/// show up without a restart).
const RESCAN: Duration = Duration::from_secs(2);

/// What the list shows: the files found, and when it looked.
#[derive(Default)]
pub(super) struct ModelList {
    pub search: String,
    files: Vec<PathBuf>,
    scanned: Option<Instant>,
}

impl Editor {
    pub(super) fn model_list(&mut self, ui: &mut Ui, output: &mut EditorOutput) {
        let list = &mut self.models;
        if list.scanned.is_none_or(|t| t.elapsed() > RESCAN) {
            list.files = all_models(&self.project_dir);
            list.scanned = Some(Instant::now());
        }
        super::section(ui, "MODELS");
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .add_enabled(!list.search.is_empty(), egui::Button::new("X").small())
                    .on_hover_text("CLEAR THE SEARCH")
                    .clicked()
                {
                    list.search.clear();
                }
                ui.add(
                    egui::TextEdit::singleline(&mut list.search)
                        .hint_text("SEARCH MODELS")
                        .desired_width(ui.available_width().max(0.0)),
                );
            });
        });
        let query = list.search.to_lowercase();
        let shown: Vec<&PathBuf> = list
            .files
            .iter()
            .filter(|p| display(p).to_lowercase().contains(&query))
            .collect();
        if list.files.is_empty() {
            ui.label(dim("NO MODELS IN MODELS/ YET"));
        } else if shown.is_empty() {
            ui.label(dim("NOTHING MATCHES"));
        }
        egui::ScrollArea::vertical()
            .id_salt("model_list")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for path in shown {
                    ui.horizontal(|ui| {
                        if ui
                            .small_button("+")
                            .on_hover_text("PLACE IN FRONT OF THE CAMERA")
                            .clicked()
                        {
                            output.place_model = Some((path.clone(), None));
                        }
                        let id = egui::Id::unique(("model_list_item", path));
                        let item = ui
                            .dnd_drag_source(id, path.clone(), |ui| {
                                ui.add(egui::Label::new(display(path)).truncate())
                            })
                            .inner;
                        item.on_hover_text("DRAG INTO THE VIEW TO PLACE IT");
                    });
                }
            });
    }

    /// A model dragged from the list and let go over the scene image is
    /// placed where it was let go.
    pub(super) fn drop_model(&mut self, ui: &Ui, image: egui::Rect, output: &mut EditorOutput) {
        let (released, pointer): (bool, Option<Pos2>) =
            ui.input(|i| (i.pointer.any_released(), i.pointer.latest_pos()));
        if !released {
            return;
        }
        if let Some(path) = egui::DragAndDrop::take_payload::<PathBuf>(ui.ctx())
            && let Some(at) = pointer.filter(|&p| image.contains(p))
        {
            output.place_model = Some(((*path).clone(), Some(at)));
        }
    }
}

/// The path inside `models/`, with `/` separators.
fn display(path: &std::path::Path) -> String {
    let inside = path.strip_prefix("models").unwrap_or(path);
    inside
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}
