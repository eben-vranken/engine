//! Gameplay graphs: the game's logic as nodes and connections, stored as
//! RON in the project's `graphs/` folder. One system for each scene's
//! logic (the player reaches the hall, a door opens) and for dialogue (a
//! line, the player's replies, the answer). Every scene has at most one
//! graph of its own (`graphs/scenes/<scene>.ron`), loaded only while the
//! scene is; flags are global, so one scene's graph can set what
//! another's checks.
//!
//! A node becomes *active* when a connection leads into it. Actions do
//! their thing and pass straight on; events and waits stay active until
//! what they wait for happens; a line stays active until the player moves
//! on. Every graph begins at its START nodes. Node types are reflected, so
//! the inspector edits their fields like any component's.

pub mod flags;
pub mod run;

use crate::scene::{
    CharacterRef, FlagRef, GraphRef, ObjectRef, SceneRef, ScriptRef, SoundRef, ZoneRef,
};
use bevy_reflect::Reflect;
use flags::Condition;
use glam::Vec2;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A node's id, unique within its graph and kept when nodes are removed,
/// so saved games can point at nodes.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct NodeId(pub u32);

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Graph {
    pub nodes: Vec<Node>,
    pub links: Vec<Link>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub id: NodeId,
    /// Where it sits on the canvas (canvas units, about points at zoom 1).
    pub position: Vec2,
    pub kind: NodeKind,
}

/// A connection from one of a node's outputs to another node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Link {
    pub from: NodeId,
    /// Which output of `from` (0 for nodes with one).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub output: u32,
    pub to: NodeId,
}

fn is_zero(value: &u32) -> bool {
    *value == 0
}

/// What a node does. Variants hold their settings as named fields, which
/// the inspector shows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Reflect)]
pub enum NodeKind {
    /// Where the graph begins.
    Start,
    /// A comment on the canvas; does nothing.
    Note { text: String },
    /// Waits until this scene is the one loaded.
    WhenInScene { scene: SceneRef },
    /// Waits until the player is in this zone.
    WhenInZone { zone: ZoneRef },
    /// Waits until the player is inside this TriggerZone object.
    WhenInTrigger { trigger: ObjectRef },
    /// Waits until the player is outside this TriggerZone object.
    WhenOutOfTrigger { trigger: ObjectRef },
    /// Waits until the player uses this object (a switch).
    WhenUsed { object: ObjectRef },
    /// Waits until the player picks up this object (a Pickup).
    WhenPickedUp { object: ObjectRef },
    /// Waits until the condition holds.
    WaitUntil { condition: Condition },
    /// Waits this many seconds.
    Wait { seconds: f32 },
    /// Goes on through YES or NO.
    Branch { condition: Condition },
    /// Sets a flag. Yes/no flags take true or false, numbers a number.
    SetFlag { flag: FlagRef, value: String },
    /// Adds to a number flag (negative to take away).
    AddToFlag { flag: FlagRef, amount: f32 },
    /// Opens, closes or toggles a Door.
    Door { door: ObjectRef, action: DoorAction },
    /// Shows or hides an object (hidden: not drawn, no collision).
    Show { object: ObjectRef, visible: bool },
    /// Plays a sound once, from an object (empty: at the player).
    PlaySound {
        sound: SoundRef,
        at: ObjectRef,
        volume: f32,
    },
    /// Loads another scene; the player starts at its Player Start.
    LoadScene { scene: SceneRef },
    /// Runs another graph (a dialogue, a side thread) and goes on once it
    /// has ended.
    RunGraph { graph: GraphRef },
    /// Changes what a character is called from now on.
    Rename {
        character: CharacterRef,
        name: String,
    },
    /// Saves the game here.
    SaveGame,
    /// Turns an ability on or off: a script in `scripts/` that runs every
    /// frame while it's on (saved with the game).
    Ability { script: ScriptRef, enabled: bool },
    /// A line of dialogue. Replies connected to its output are offered to
    /// the player; anything else connected runs when the line is left.
    Line {
        speaker: CharacterRef,
        text: String,
        /// Played when the line appears; empty = silent.
        sound: SoundRef,
    },
    /// A reply the player can pick, connected from a line.
    Reply { text: String, condition: Condition },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, Reflect)]
pub enum DoorAction {
    #[default]
    Open,
    Close,
    Toggle,
}

/// How the add menu groups node types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Story,
    Logic,
    Action,
    Dialogue,
}

impl Category {
    pub const ALL: [Category; 4] = [
        Category::Story,
        Category::Logic,
        Category::Action,
        Category::Dialogue,
    ];

    /// One node's category, for its header.
    pub fn tag(self) -> &'static str {
        match self {
            Category::Story => "EVENT",
            Category::Logic => "LOGIC",
            Category::Action => "ACTION",
            Category::Dialogue => "DIALOGUE",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Category::Story => "EVENTS",
            Category::Logic => "LOGIC",
            Category::Action => "ACTIONS",
            Category::Dialogue => "DIALOGUE",
        }
    }
}

impl NodeKind {
    /// One of every node type, with starting values (the add menu).
    pub fn all() -> Vec<NodeKind> {
        use NodeKind::*;
        vec![
            Start,
            Note {
                text: String::new(),
            },
            WhenInScene {
                scene: SceneRef::default(),
            },
            WhenInZone {
                zone: ZoneRef::default(),
            },
            WhenInTrigger {
                trigger: ObjectRef::default(),
            },
            WhenOutOfTrigger {
                trigger: ObjectRef::default(),
            },
            WhenUsed {
                object: ObjectRef::default(),
            },
            WhenPickedUp {
                object: ObjectRef::default(),
            },
            Wait { seconds: 1.0 },
            WaitUntil {
                condition: Condition::default(),
            },
            Branch {
                condition: Condition::default(),
            },
            SetFlag {
                flag: FlagRef::default(),
                value: "true".into(),
            },
            AddToFlag {
                flag: FlagRef::default(),
                amount: 1.0,
            },
            Door {
                door: ObjectRef::default(),
                action: DoorAction::Open,
            },
            Show {
                object: ObjectRef::default(),
                visible: true,
            },
            PlaySound {
                sound: SoundRef::default(),
                at: ObjectRef::default(),
                volume: 1.0,
            },
            LoadScene {
                scene: SceneRef::default(),
            },
            RunGraph {
                graph: GraphRef::default(),
            },
            Rename {
                character: CharacterRef::default(),
                name: String::new(),
            },
            SaveGame,
            Ability {
                script: ScriptRef::default(),
                enabled: true,
            },
            Line {
                speaker: CharacterRef::default(),
                text: String::new(),
                sound: SoundRef::default(),
            },
            Reply {
                text: String::new(),
                condition: Condition::default(),
            },
        ]
    }

    pub fn category(&self) -> Category {
        use NodeKind::*;
        match self {
            Start | WhenInScene { .. } | WhenInZone { .. } | WhenInTrigger { .. } => {
                Category::Story
            }
            WhenOutOfTrigger { .. } | WhenUsed { .. } | WhenPickedUp { .. } => Category::Story,
            Note { .. } | Wait { .. } | WaitUntil { .. } | Branch { .. } | RunGraph { .. } => {
                Category::Logic
            }
            Line { .. } | Reply { .. } | Rename { .. } => Category::Dialogue,
            _ => Category::Action,
        }
    }

    /// The node's name on the canvas and in menus.
    pub fn title(&self) -> &'static str {
        use NodeKind::*;
        match self {
            Start => "START",
            Note { .. } => "NOTE",
            WhenInScene { .. } => "WHEN IN SCENE",
            WhenInZone { .. } => "WHEN IN ZONE",
            WhenInTrigger { .. } => "WHEN IN TRIGGER",
            WhenOutOfTrigger { .. } => "WHEN OUT OF TRIGGER",
            WhenUsed { .. } => "WHEN USED",
            WhenPickedUp { .. } => "WHEN PICKED UP",
            WaitUntil { .. } => "WAIT UNTIL",
            Wait { .. } => "WAIT",
            Branch { .. } => "BRANCH",
            SetFlag { .. } => "SET FLAG",
            AddToFlag { .. } => "ADD TO FLAG",
            Door { .. } => "DOOR",
            Show { .. } => "SHOW / HIDE",
            PlaySound { .. } => "PLAY SOUND",
            LoadScene { .. } => "LOAD SCENE",
            RunGraph { .. } => "RUN GRAPH",
            Rename { .. } => "RENAME",
            SaveGame => "SAVE GAME",
            Ability { .. } => "ABILITY",
            Line { .. } => "LINE",
            Reply { .. } => "REPLY",
        }
    }

    /// What it does, in a line (the inspector shows it).
    pub fn description(&self) -> &'static str {
        use NodeKind::*;
        match self {
            Start => "Where the graph begins: everything connected runs at once.",
            Note { .. } => "A comment on the canvas. Does nothing.",
            WhenInScene { .. } => "Waits until this scene is the one loaded.",
            WhenInZone { .. } => "Waits until the player is in this zone (also if already there).",
            WhenInTrigger { .. } => "Waits until the player is inside this trigger zone.",
            WhenOutOfTrigger { .. } => "Waits until the player is outside this trigger zone.",
            WhenUsed { .. } => "Waits until the player presses E at this switch.",
            WhenPickedUp { .. } => "Waits until the player takes this pickup.",
            WaitUntil { .. } => "Waits until the condition holds.",
            Wait { .. } => "Waits this many seconds.",
            Branch { .. } => "Goes on through YES or NO, by the condition.",
            SetFlag { .. } => "Sets a flag, then goes on at once.",
            AddToFlag { .. } => "Adds to a number flag, then goes on at once.",
            Door { .. } => "Opens, closes or toggles a door, then goes on at once.",
            Show { .. } => "Shows or hides an object (hidden: not drawn, no collision).",
            PlaySound { .. } => "Plays a sound once from an object, or at the player.",
            LoadScene { .. } => "Loads another scene; the player starts at its Player Start.",
            RunGraph { .. } => "Runs another graph and goes on once it has ended.",
            Rename { .. } => "Changes what a character is called from now on.",
            SaveGame => "Saves the game here.",
            Ability { .. } => {
                "Turns an ability on or off: a script that runs every frame while it's on."
            }
            Line { .. } => {
                "A line of dialogue. Replies connected to it are offered; anything else connected runs when it's left."
            }
            Reply { .. } => "A reply the player can pick, connected from a line.",
        }
    }

    /// Whether connections can lead into it.
    pub fn has_input(&self) -> bool {
        !matches!(self, NodeKind::Start | NodeKind::Note { .. })
    }

    /// Its outputs' names.
    pub fn outputs(&self) -> &'static [&'static str] {
        match self {
            NodeKind::Note { .. } => &[],
            NodeKind::Branch { .. } => &["YES", "NO"],
            _ => &["NEXT"],
        }
    }

    /// The objects it names (by name).
    pub fn objects(&self) -> Vec<&str> {
        use NodeKind::*;
        match self {
            WhenInTrigger { trigger: o } | WhenOutOfTrigger { trigger: o } => vec![o.0.as_str()],
            WhenUsed { object: o } | WhenPickedUp { object: o } | Show { object: o, .. } => {
                vec![o.0.as_str()]
            }
            Door { door, .. } => vec![door.0.as_str()],
            PlaySound { at, .. } => vec![at.0.as_str()],
            _ => Vec::new(),
        }
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect()
    }

    /// The character it names (a speaker, a rename).
    pub fn character(&self) -> Option<&str> {
        match self {
            NodeKind::Line { speaker: c, .. } | NodeKind::Rename { character: c, .. } => {
                Some(c.0.as_str()).filter(|s| !s.is_empty())
            }
            _ => None,
        }
    }

    /// The flags it reads or sets.
    pub fn flags(&self) -> Vec<&str> {
        use NodeKind::*;
        match self {
            WaitUntil { condition } | Branch { condition } | Reply { condition, .. } => {
                vec![condition.flag.0.as_str()]
            }
            SetFlag { flag, .. } | AddToFlag { flag, .. } => vec![flag.0.as_str()],
            _ => Vec::new(),
        }
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect()
    }

    /// Project files it names: (what, path).
    pub fn files(&self) -> Vec<(&'static str, &str)> {
        use NodeKind::*;
        match self {
            WhenInScene { scene } | LoadScene { scene } => vec![("SCENE", scene.0.as_str())],
            RunGraph { graph } => vec![("GRAPH", graph.0.as_str())],
            Ability { script, .. } => vec![("SCRIPT", script.0.as_str())],
            PlaySound { sound, .. } | Line { sound, .. } => vec![("SOUND", sound.0.as_str())],
            _ => Vec::new(),
        }
        .into_iter()
        .filter(|(_, s)| !s.is_empty())
        .collect()
    }

    /// A few short lines describing its settings, for the canvas.
    pub fn summary(&self) -> Vec<String> {
        use NodeKind::*;
        let or_none = |s: &str| {
            if s.is_empty() {
                "-".to_owned()
            } else {
                s.to_owned()
            }
        };
        match self {
            Start | SaveGame => Vec::new(),
            Note { text } | Reply { text, .. } if !text.is_empty() => {
                let mut lines = wrap(text, 26, 3);
                if let Reply { condition, .. } = self
                    && !condition.flag.0.is_empty()
                {
                    lines.push(format!("IF {}", condition.describe()));
                }
                lines
            }
            Note { .. } | Reply { .. } => vec!["-".into()],
            WhenInScene { scene } | LoadScene { scene } => vec![or_none(&scene.0)],
            WhenInZone { zone } => vec![or_none(&zone.0)],
            WhenInTrigger { trigger } | WhenOutOfTrigger { trigger } => vec![or_none(&trigger.0)],
            WhenUsed { object } | WhenPickedUp { object } => vec![or_none(&object.0)],
            WaitUntil { condition } | Branch { condition } => vec![condition.describe()],
            Wait { seconds } => vec![format!("{seconds} S")],
            SetFlag { flag, value } => vec![format!("{} = {value}", or_none(&flag.0))],
            AddToFlag { flag, amount } => vec![format!("{} + {amount}", or_none(&flag.0))],
            Door { door, action } => {
                vec![format!("{action:?} {}", or_none(&door.0)).to_uppercase()]
            }
            Show { object, visible } => {
                let what = if *visible { "SHOW" } else { "HIDE" };
                vec![format!("{what} {}", or_none(&object.0))]
            }
            PlaySound { sound, at, .. } => {
                let at = if at.0.is_empty() { "PLAYER" } else { &at.0 };
                vec![or_none(&sound.0), format!("AT {at}")]
            }
            RunGraph { graph } => vec![or_none(&graph.0)],
            Ability { script, enabled } => {
                let what = if *enabled { "ON" } else { "OFF" };
                vec![format!("{what} {}", or_none(&script_name(&script.0)))]
            }
            Rename { character, name } => vec![format!("{} → {}", or_none(&character.0), name)],
            Line {
                speaker,
                text,
                sound,
            } => {
                let mut lines = vec![format!("{}:", or_none(&speaker.0).to_uppercase())];
                lines.extend(wrap(text, 26, 3));
                if !sound.0.is_empty() {
                    lines.push(format!("♪ {}", sound.0));
                }
                lines
            }
        }
    }
}

/// A script's short name: `scripts/double_jump.rhai` → `double_jump`.
pub fn script_name(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Breaks `text` into lines of at most `width` characters (at spaces), at
/// most `max` lines; the last ends in `…` if text was left out.
fn wrap(text: &str, width: usize, max: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    if lines.len() > max {
        lines.truncate(max);
        if let Some(last) = lines.last_mut() {
            last.push('…');
        }
    }
    lines
}

#[derive(Debug, thiserror::Error)]
pub enum GraphError {
    #[error("could not read {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("could not write {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{path}: {source}")]
    Parse {
        path: PathBuf,
        source: Box<ron::error::SpannedError>,
    },
}

impl Graph {
    /// A new graph: just a START node.
    pub fn starter() -> Self {
        Graph {
            nodes: vec![Node {
                id: NodeId(1),
                position: Vec2::new(40.0, 120.0),
                kind: NodeKind::Start,
            }],
            links: Vec::new(),
        }
    }

    pub fn load(path: &Path) -> Result<Self, GraphError> {
        let text = std::fs::read_to_string(path).map_err(|source| GraphError::Read {
            path: path.to_owned(),
            source,
        })?;
        ron::from_str(&text).map_err(|source| GraphError::Parse {
            path: path.to_owned(),
            source: Box::new(source),
        })
    }

    pub fn save(&self, path: &Path) -> Result<(), GraphError> {
        let config = ron::ser::PrettyConfig::default().struct_names(false);
        let text = ron::ser::to_string_pretty(self, config).expect("graphs always serialize");
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::fs::write(path, text).map_err(|source| GraphError::Write {
            path: path.to_owned(),
            source,
        })
    }

    pub fn node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }

    pub fn node_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        self.nodes.iter_mut().find(|n| n.id == id)
    }

    /// An id no node has (nor had, as long as the highest stays).
    pub fn free_id(&self) -> NodeId {
        NodeId(self.nodes.iter().map(|n| n.id.0).max().unwrap_or(0) + 1)
    }

    /// Adds a node; returns its id.
    pub fn add(&mut self, kind: NodeKind, position: Vec2) -> NodeId {
        let id = self.free_id();
        self.nodes.push(Node { id, position, kind });
        id
    }

    /// Removes a node and its connections.
    pub fn remove(&mut self, id: NodeId) {
        self.nodes.retain(|n| n.id != id);
        self.links.retain(|l| l.from != id && l.to != id);
    }

    /// Connects `from`'s output to `to`, unless that makes no sense (no
    /// such output or input, a node to itself) or exists already. Returns
    /// whether it connected.
    pub fn connect(&mut self, from: NodeId, output: u32, to: NodeId) -> bool {
        let link = Link { from, output, to };
        let valid = from != to
            && self
                .node(from)
                .is_some_and(|n| (output as usize) < n.kind.outputs().len())
            && self.node(to).is_some_and(|n| n.kind.has_input())
            && !self.links.contains(&link);
        if valid {
            self.links.push(link);
        }
        valid
    }

    /// The nodes `from`'s output leads to, in the order connected.
    pub fn targets(&self, from: NodeId, output: u32) -> impl Iterator<Item = NodeId> + '_ {
        self.links
            .iter()
            .filter(move |l| l.from == from && l.output == output)
            .map(|l| l.to)
    }

    /// The START nodes.
    pub fn starts(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.nodes
            .iter()
            .filter(|n| n.kind == NodeKind::Start)
            .map(|n| n.id)
    }
}

/// Where a scene's own graph lives: `graphs/scenes/<scene name>.ron`. It
/// starts when the scene loads, pauses when the player leaves and carries
/// on when they return.
pub fn scene_graph(scene: &str) -> String {
    let stem = Path::new(scene)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    format!("graphs/scenes/{stem}.ron")
}

/// The scene a scene graph belongs to, if `graph` is one.
pub fn scene_of_graph(graph: &str) -> Option<String> {
    let stem = graph.strip_prefix("graphs/scenes/")?.strip_suffix(".ron")?;
    Some(format!("scenes/{stem}.ron"))
}

/// A project path as graphs and saves write it: relative, with `/`.
pub fn key(path: &Path) -> String {
    path.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connections_are_checked() {
        let mut graph = Graph::starter();
        let start = NodeId(1);
        let branch = graph.add(
            NodeKind::Branch {
                condition: Condition::default(),
            },
            Vec2::ZERO,
        );
        let note = graph.add(NodeKind::Note { text: "hi".into() }, Vec2::ZERO);
        assert!(graph.connect(start, 0, branch));
        assert!(!graph.connect(start, 0, branch), "already connected");
        assert!(!graph.connect(start, 1, branch), "START has one output");
        assert!(!graph.connect(branch, 1, branch), "not to itself");
        assert!(!graph.connect(branch, 0, start), "START has no input");
        assert!(!graph.connect(branch, 0, note), "notes have no input");
        assert_eq!(graph.targets(start, 0).collect::<Vec<_>>(), vec![branch]);

        graph.remove(branch);
        assert!(graph.links.is_empty());
        assert_eq!(graph.free_id(), NodeId(4), "ids aren't reused");
    }

    #[test]
    fn graphs_save_and_load() {
        let mut graph = Graph::starter();
        let line = graph.add(
            NodeKind::Line {
                speaker: CharacterRef("keeper".into()),
                text: "You found the hall.".into(),
                sound: SoundRef::default(),
            },
            Vec2::new(200.0, 0.0),
        );
        graph.connect(NodeId(1), 0, line);
        let dir = std::env::temp_dir().join(format!("engine-graph-{}", std::process::id()));
        let file = dir.join("graphs/test.ron");
        graph.save(&file).unwrap();
        assert_eq!(Graph::load(&file).unwrap(), graph);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn every_node_type_is_listed_once() {
        let all = NodeKind::all();
        let mut titles: Vec<_> = all.iter().map(|k| k.title()).collect();
        titles.sort();
        titles.dedup();
        assert_eq!(titles.len(), all.len());
    }

    #[test]
    fn text_wraps() {
        assert_eq!(wrap("a bb ccc", 4, 5), vec!["a bb", "ccc"]);
        assert_eq!(wrap("one two three", 3, 2), vec!["one", "two…"]);
    }
}
