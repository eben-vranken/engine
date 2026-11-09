//! Renaming something other files refer to by name (a flag, a zone):
//! every reference in the project follows. Open documents change in memory
//! (an undo step in a scene, unsaved in a graph), files that aren't open are
//! rewritten; the message lists what changed.

use super::Editor;
use super::files::{graph_files, scene_files};
use super::history::Edit;
use crate::graph::Graph;
use crate::scene::Scene;

/// What is being renamed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Renamed {
    Flag,
    /// A zone of one scene (`scene`, a project path): its own graph and the
    /// graphs that aren't a scene's follow.
    Zone,
}

fn in_graph(kind: Renamed, graph: &mut Graph, old: &str, new: &str) -> bool {
    match kind {
        Renamed::Flag => graph.rename_flag(old, new),
        Renamed::Zone => graph.rename_zone(old, new),
    }
}

impl Editor {
    /// Renames `old` to `new` everywhere in the project except the active
    /// scene, which the caller changes as its own undo step (`scene` is it,
    /// as the caller left it). For a zone, `zone_scene` is the scene the
    /// zone belongs to.
    pub(super) fn rename_references(
        &mut self,
        kind: Renamed,
        old: &str,
        new: &str,
        scene: &mut Scene,
        zone_scene: Option<&str>,
    ) {
        if old == new || old.is_empty() || new.is_empty() {
            return;
        }
        let mut touched: Vec<String> = Vec::new();
        // Which graphs a rename reaches: for a zone, its scene's graph and
        // the graphs that belong to no scene.
        let reaches = |graph: &str| match (kind, zone_scene) {
            (Renamed::Zone, Some(scene)) => match crate::graph::scene_of_graph(graph) {
                Some(owner) => owner == scene,
                None => true,
            },
            _ => true,
        };
        let active = crate::graph::key(&self.scene_path);
        // The active tab.
        if kind == Renamed::Flag && scene.rename_flag(old, new) {
            touched.push(active.clone());
        }
        if let Some(doc) = &mut self.graph
            && reaches(&active)
            && in_graph(kind, &mut doc.graph, old, new)
        {
            touched.push(active.clone());
        }
        // Parked tabs.
        let mut open = vec![active];
        for parked in self.tabs.iter_mut().flatten() {
            let path = crate::graph::key(&parked.doc.scene_path);
            open.push(path.clone());
            if let Some(doc) = &mut parked.doc.graph {
                if reaches(&path) && in_graph(kind, &mut doc.graph, old, new) {
                    touched.push(path);
                }
            } else if kind == Renamed::Flag && parked.doc.script.is_none() {
                let before = parked.scene.clone();
                if parked.scene.rename_flag(old, new) {
                    parked.doc.history.record(Edit::SetScene {
                        before: Box::new(before),
                        after: Box::new(parked.scene.clone()),
                    });
                    touched.push(path);
                }
            }
        }
        // Files that aren't open.
        for path in graph_files(&self.project_dir) {
            let key = crate::graph::key(&path);
            if open.contains(&key) || !reaches(&key) {
                continue;
            }
            let file = self.project_dir.join(&path);
            if let Ok(mut graph) = Graph::load(&file)
                && in_graph(kind, &mut graph, old, new)
            {
                match graph.save(&file) {
                    Ok(()) => touched.push(key),
                    Err(err) => self.show_error(err.to_string()),
                }
            }
        }
        if kind == Renamed::Flag {
            for path in scene_files(&self.project_dir) {
                let key = crate::graph::key(&path);
                if open.contains(&key) {
                    continue;
                }
                let file = self.project_dir.join(&path);
                if let Ok(mut other) = Scene::load(&file)
                    && other.rename_flag(old, new)
                {
                    match other.save(&file) {
                        Ok(()) => touched.push(key),
                        Err(err) => self.show_error(err.to_string()),
                    }
                }
            }
        }
        self.index.refresh(&self.project_dir, true);
        touched.dedup();
        if !touched.is_empty() {
            self.show_message(
                format!("\"{old}\" → \"{new}\" IN {}", touched.join(", ")).to_uppercase(),
            );
        }
    }
}
