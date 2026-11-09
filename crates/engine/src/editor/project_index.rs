//! What the editor knows about the whole project, looked at again every
//! few seconds: flags, graphs, scenes, sounds, and the objects, zones and
//! characters of every scene. Reference pickers offer these, and the
//! logic panels use them to show who owns a graph, what uses it, what
//! mentions an object, and references that point at nothing.

use super::files::{graph_files, scene_files, script_files, sound_files};
use super::inspector::Names;
use crate::graph::flags::FlagDefs;
use crate::graph::{Graph, NodeKind, key, scene_graph, scene_of_graph};
use crate::scene::{Component, Scene};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

const RESCAN: Duration = Duration::from_secs(3);

/// Who a graph belongs to, which says when it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Owner {
    /// A scene's own graph: runs while that scene is loaded.
    Scene(String),
    /// A character's dialogue: runs when the player talks to them.
    Character { id: String, scene: String },
    /// Started by another graph's RUN GRAPH, or by nothing yet.
    Other,
}

impl Owner {
    pub fn group(&self) -> &'static str {
        match self {
            Owner::Scene(_) => "SCENES",
            Owner::Character { .. } => "CHARACTERS",
            Owner::Other => "OTHER",
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Owner::Scene(scene) => format!("SCENE {scene}"),
            Owner::Character { id, scene } => format!("CHARACTER {id} IN {scene}"),
            Owner::Other => "NONE: STARTED BY RUN GRAPH".into(),
        }
    }

    pub fn runs(&self) -> &'static str {
        match self {
            Owner::Scene(_) => {
                "WHILE THE SCENE IS LOADED; CARRIES ON WHERE IT WAS WHEN THE PLAYER COMES BACK"
            }
            Owner::Character { .. } => "WHEN THE PLAYER TALKS TO THE CHARACTER (E)",
            Owner::Other => "WHEN A RUN GRAPH NODE STARTS IT",
        }
    }
}

/// Something that uses a graph, and where to go to see it.
#[derive(Debug, Clone, PartialEq)]
pub struct Use {
    pub text: String,
    pub go: Go,
}

/// A place in the project the editor can open.
#[derive(Debug, Clone, PartialEq)]
pub enum Go {
    Scene(String),
    Graph(String),
}

#[derive(Default)]
pub(super) struct ProjectIndex {
    scanned: Option<Instant>,
    pub flags: FlagDefs,
    pub names: Names,
    /// `flags` was edited and isn't written yet (a rescan keeps it).
    pub flags_unsaved: bool,
    /// Every graph, by project path.
    pub graphs: BTreeMap<String, Graph>,
    /// Every scene's objects, by project path.
    pub scene_objects: BTreeMap<String, Vec<String>>,
    /// Characters: (scene, object name, id, dialogue graph).
    pub characters: Vec<(String, String, String, String)>,
    /// Objects with a Script behavior: (scene, object name, script).
    pub scripted: Vec<(String, String, String)>,
    /// Each character id's name before any RENAME.
    pub character_names: std::collections::HashMap<String, String>,
    /// How dialogue looks (`game.ron`).
    pub dialogue: crate::dialogue::DialogueStyle,
}

impl ProjectIndex {
    /// Looks at the project again if it's been a while (or `force`).
    pub fn refresh(&mut self, dir: &Path, force: bool) {
        if !force && self.scanned.is_some_and(|t| t.elapsed() < RESCAN) {
            return;
        }
        self.scanned = Some(Instant::now());
        if !self.flags_unsaved {
            self.flags = FlagDefs::load(dir);
        }
        let settings = crate::project::Project::open(dir).ok().map(|p| p.settings);
        self.dialogue = settings.map(|s| s.dialogue).unwrap_or_default();
        self.character_names.clear();
        let scenes = scene_files(dir);
        let graph_paths = graph_files(dir);
        self.graphs = graph_paths
            .iter()
            .filter_map(|p| Some((key(p), Graph::load(&dir.join(p)).ok()?)))
            .collect();
        let mut names = Names {
            flags: self.flags.names(),
            graphs: graph_paths.iter().map(|p| key(p)).collect(),
            scenes: scenes.iter().map(|p| key(p)).collect(),
            sounds: sound_files(dir),
            scripts: script_files(dir).iter().map(|p| key(p)).collect(),
            ..Default::default()
        };
        self.scene_objects.clear();
        self.characters.clear();
        self.scripted.clear();
        for path in &scenes {
            let Ok(scene) = Scene::load(&dir.join(path)) else {
                continue;
            };
            let scene_key = key(path);
            names
                .zones
                .extend(scene.zones.iter().map(|z| z.name.clone()));
            let mut objects = Vec::new();
            for object in &scene.objects {
                names.objects.push(object.name.clone());
                objects.push(object.name.clone());
                for component in &object.components {
                    if let Component::Script(s) = component
                        && !s.script.0.is_empty()
                    {
                        self.scripted.push((
                            scene_key.clone(),
                            object.name.clone(),
                            s.script.0.clone(),
                        ));
                    }
                    if let Component::Character(c) = component
                        && !c.id.is_empty()
                    {
                        names.characters.push(c.id.clone());
                        self.character_names.insert(c.id.clone(), c.name.clone());
                        self.characters.push((
                            scene_key.clone(),
                            object.name.clone(),
                            c.id.clone(),
                            c.dialogue.0.clone(),
                        ));
                    }
                }
            }
            self.scene_objects.insert(scene_key, objects);
        }
        for list in [
            &mut names.objects,
            &mut names.zones,
            &mut names.characters,
            &mut names.flags,
        ] {
            list.sort();
            list.dedup();
        }
        self.names = names;
    }

    /// The pickers' lists for editing `scene`: its own objects and zones,
    /// the rest from the project.
    pub fn for_scene(&self, scene: &Scene) -> Names {
        let mut objects: Vec<String> = scene.objects.iter().map(|o| o.name.clone()).collect();
        objects.sort();
        Names {
            objects,
            zones: scene.zones.iter().map(|z| z.name.clone()).collect(),
            ..self.names.clone()
        }
    }

    pub fn owner(&self, graph: &str) -> Owner {
        if let Some(scene) = scene_of_graph(graph) {
            return Owner::Scene(scene);
        }
        if let Some((scene, _, id, _)) = self.characters.iter().find(|c| c.3 == graph) {
            return Owner::Character {
                id: id.clone(),
                scene: scene.clone(),
            };
        }
        Owner::Other
    }

    /// What uses a script: ABILITY nodes in graphs and objects with its
    /// Script behavior, as (text, where to go).
    pub fn script_users(&self, script: &str) -> Vec<(String, Go)> {
        let mut uses = Vec::new();
        for (path, graph) in &self.graphs {
            let turns_on = graph
                .nodes
                .iter()
                .any(|n| matches!(&n.kind, NodeKind::Ability { script: s, .. } if s.0 == script));
            if turns_on {
                uses.push((format!("ABILITY · {path}"), Go::Graph(path.clone())));
            }
        }
        for (scene, object, s) in &self.scripted {
            if s == script {
                uses.push((format!("{object} · {scene}"), Go::Scene(scene.clone())));
            }
        }
        uses
    }

    /// Everything that uses `graph`.
    pub fn used_by(&self, graph: &str) -> Vec<Use> {
        let mut uses = Vec::new();
        if let Some(scene) = scene_of_graph(graph)
            && self.scene_objects.contains_key(&scene)
        {
            uses.push(Use {
                text: format!("{scene} · OWNER"),
                go: Go::Scene(scene),
            });
        }
        for (scene, object, _, dialogue) in &self.characters {
            if dialogue == graph {
                uses.push(Use {
                    text: format!("{object} (CHARACTER) · {scene}"),
                    go: Go::Scene(scene.clone()),
                });
            }
        }
        for (path, other) in &self.graphs {
            let runs_it = other
                .nodes
                .iter()
                .any(|n| matches!(&n.kind, NodeKind::RunGraph { graph: g } if g.0 == graph));
            if runs_it {
                uses.push(Use {
                    text: format!("{path} · RUN GRAPH"),
                    go: Go::Graph(path.clone()),
                });
            }
        }
        uses
    }

    /// Graphs that mention an object of `scene` (or the character it is):
    /// (graph, what).
    pub fn mentions(
        &self,
        scene: &str,
        object: &str,
        character: Option<&str>,
    ) -> Vec<(String, String)> {
        let own_graph = scene_graph(scene);
        let mut found = Vec::new();
        if let Some(id) = character {
            for (_, _, cid, dialogue) in &self.characters {
                if cid == id && !dialogue.is_empty() && !found.iter().any(|(p, _)| p == dialogue) {
                    found.push((dialogue.clone(), "ITS DIALOGUE".to_string()));
                }
            }
        }
        for (path, graph) in &self.graphs {
            // A scene graph only means its own scene's objects.
            let other_scene = path.starts_with("graphs/scenes/") && *path != own_graph;
            let mut what: Vec<&str> = Vec::new();
            for node in &graph.nodes {
                let names_object = !other_scene && node.kind.objects().contains(&object);
                let names_character = character.is_some() && node.kind.character() == character;
                if (names_object || names_character) && !what.contains(&node.kind.title()) {
                    what.push(node.kind.title());
                }
            }
            if !what.is_empty() && !found.iter().any(|(p, _)| p == path) {
                found.push((path.clone(), what.join(", ")));
            }
        }
        found
    }

    /// References in `graph` that point at nothing: objects missing from
    /// its scene (or every scene), unknown flags, missing files.
    pub fn problems(&self, path: &str, graph: &Graph) -> Vec<String> {
        let own_scene = scene_of_graph(path);
        let all_objects: Vec<&String> = self.scene_objects.values().flatten().collect();
        let mut problems = Vec::new();
        for node in &graph.nodes {
            let title = node.kind.title();
            for object in node.kind.objects() {
                let known = match own_scene.as_ref().and_then(|s| self.scene_objects.get(s)) {
                    Some(objects) => objects.iter().any(|o| o == object),
                    None => all_objects.iter().any(|o| *o == object),
                };
                if !known {
                    problems.push(format!("{title}: NO OBJECT \"{object}\""));
                }
            }
            if let Some(c) = node.kind.character()
                && !self.names.characters.iter().any(|k| k == c)
            {
                problems.push(format!("{title}: NO CHARACTER \"{c}\""));
            }
            for flag in node.kind.flags() {
                if !self.names.flags.iter().any(|f| f == flag) {
                    problems.push(format!("{title}: FLAG \"{flag}\" ISN'T DECLARED"));
                }
            }
            for (what, file) in node.kind.files() {
                let list = match what {
                    "SCENE" => &self.names.scenes,
                    "GRAPH" => &self.names.graphs,
                    "SCRIPT" => &self.names.scripts,
                    _ => &self.names.sounds,
                };
                if !list.iter().any(|f| f == file) {
                    problems.push(format!("{title}: NO {what} \"{file}\""));
                }
            }
        }
        problems.dedup();
        problems
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sandbox_graphs_have_owners_and_uses() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sandbox");
        let mut index = ProjectIndex::default();
        index.refresh(&dir, true);
        assert_eq!(
            index.owner("graphs/scenes/test_level.ron"),
            Owner::Scene("scenes/test_level.ron".into())
        );
        assert!(matches!(
            index.owner("graphs/keeper.ron"),
            Owner::Character { .. }
        ));
        assert!(!index.used_by("graphs/keeper.ron").is_empty());
        for (path, graph) in &index.graphs {
            let problems = index.problems(path, graph);
            assert!(problems.is_empty(), "{path}: {problems:?}");
        }
    }
}
