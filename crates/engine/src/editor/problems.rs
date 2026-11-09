//! What's wrong, in one place: an object's own problems (marked with `!` in
//! the outliner) and the project's (references in graphs that point at
//! nothing), listed together by PROBLEMS.

use super::{Editor, dim};
use crate::scene::{Component, Object, Scene};
use crate::ui::theme;
use egui::{RichText, Ui};

impl Editor {
    /// An object's problems: links to nothing, unknown flags, two behaviors
    /// that both move it.
    pub(super) fn object_problems(&self, scene: &Scene, object: &Object) -> Vec<String> {
        let mut problems = Vec::new();
        let flags = &self.index.names.flags;
        let unknown = |flag: &str| !flag.is_empty() && !flags.iter().any(|f| f == flag);
        let has = |f: fn(&Component) -> bool| object.components.iter().any(f);
        if has(|c| matches!(c, Component::Script(_)))
            && has(|c| matches!(c, Component::Door(_) | Component::PhysicsBody(_)))
        {
            problems.push("A SCRIPT AND A DOOR OR PHYSICS BODY BOTH MOVE IT".into());
        }
        for component in &object.components {
            match component {
                Component::Switch(switch)
                    if !switch.target.is_empty() && scene.resolve(&switch.target).is_none() =>
                {
                    problems.push(format!("SWITCH: NO OBJECT \"{}\"", switch.target.name));
                }
                Component::Pickup(pickup) if unknown(&pickup.flag.0) => {
                    problems.push(format!("PICKUP: NO FLAG \"{}\"", pickup.flag.0));
                }
                Component::VisibleWhen(when) if unknown(&when.condition.flag.0) => {
                    problems.push(format!(
                        "VISIBLE WHEN: NO FLAG \"{}\"",
                        when.condition.flag.0
                    ));
                }
                Component::Character(character)
                    if !character.dialogue.0.is_empty()
                        && !self.index.names.graphs.contains(&character.dialogue.0) =>
                {
                    problems.push(format!("CHARACTER: NO GRAPH \"{}\"", character.dialogue.0));
                }
                _ => {}
            }
        }
        problems
    }

    /// Every problem: (where, what), the open scene's objects first.
    pub(super) fn all_problems(&self, scene: &Scene) -> Vec<(Place, String)> {
        let mut all = Vec::new();
        if self.graph.is_none() && self.script.is_none() {
            for (index, object) in scene.objects.iter().enumerate() {
                for problem in self.object_problems(scene, object) {
                    all.push((Place::Object(index, object.name.clone()), problem));
                }
            }
        }
        for (path, graph) in &self.index.graphs {
            for problem in self.index.problems(path, graph) {
                all.push((Place::Graph(path.clone()), problem));
            }
        }
        all
    }

    /// PROBLEMS: the list; a row opens what it is about.
    pub(super) fn problems_ui(&mut self, ui: &Ui, scene: &Scene, output: &mut super::EditorOutput) {
        if !self.problems_open {
            return;
        }
        let problems = self.all_problems(scene);
        let mut go = None;
        let modal = egui::Modal::new(egui::Id::unique("problems")).show(ui.ctx(), |ui| {
            ui.set_width(640.0);
            ui.label(RichText::new(format!("PROBLEMS · {}", problems.len())).strong());
            ui.add_space(6.0);
            if problems.is_empty() {
                ui.label(dim("NOTHING POINTS AT NOTHING"));
            }
            egui::ScrollArea::vertical()
                .max_height(420.0)
                .show(ui, |ui| {
                    for (place, problem) in &problems {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("!").color(theme::ALERT));
                            if ui.link(place.title()).clicked() {
                                go = Some(place.clone());
                            }
                            ui.label(RichText::new(problem).color(theme::AMBER_DIM));
                        });
                    }
                });
            ui.add_space(8.0);
            if ui.button("CLOSE").clicked() {
                ui.close();
            }
        });
        if modal.should_close() {
            self.problems_open = false;
        }
        match go {
            Some(Place::Object(index, _)) => {
                self.problems_open = false;
                output.settled |= self.select(scene, Some(index));
                self.reveal_selection = true;
            }
            Some(Place::Graph(path)) => {
                self.problems_open = false;
                output.open_graph = Some(path.into());
            }
            None => {}
        }
    }
}

/// Where a problem is.
#[derive(Debug, Clone)]
pub(super) enum Place {
    /// An object of the open scene: index and name.
    Object(usize, String),
    /// A graph file.
    Graph(String),
}

impl Place {
    fn title(&self) -> String {
        match self {
            Place::Object(_, name) => name.to_uppercase(),
            Place::Graph(path) => path.to_uppercase(),
        }
    }
}
