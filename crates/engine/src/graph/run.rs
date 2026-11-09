//! Running graphs. The runner holds the graphs in play (each a *thread*
//! with its active nodes), the flags and the characters' names. Each frame
//! the game tells it what the world looks like ([`World`]); it moves the
//! graphs on and hands back what the world should do ([`Command`]). It
//! knows nothing about scenes, rendering or input, so it can be tested on
//! its own.

use super::flags::Flags;
use super::{DoorAction, Graph, NodeId, NodeKind};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

/// What the world looks like this frame.
#[derive(Debug, Default, Clone, Copy)]
pub struct World<'a> {
    /// The loaded scene, as a project path with `/`.
    pub scene: &'a str,
    /// The zone the player is in.
    pub zone: Option<&'a str>,
    /// TriggerZone objects the player is inside.
    pub triggers: &'a [String],
    /// Objects the player used this frame.
    pub used: &'a [String],
    /// Objects the player picked up this frame.
    pub picked: &'a [String],
}

/// Something the world should do.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Door {
        door: String,
        action: DoorAction,
    },
    Show {
        object: String,
        visible: bool,
    },
    /// A sound once, from an object (empty: at the player).
    PlaySound {
        sound: String,
        at: String,
        volume: f32,
    },
    /// A line's sound, from its speaker (a character id).
    LineSound {
        sound: String,
        speaker: String,
    },
    LoadScene(String),
    Save,
}

/// The line on screen.
#[derive(Debug, Clone, PartialEq)]
pub struct LineView {
    /// Changes whenever another line is shown.
    pub serial: u64,
    /// The speaker's character id (the game looks up the name).
    pub speaker: String,
    pub text: String,
    /// The replies on offer; empty = the player just moves on.
    pub replies: Vec<String>,
}

struct Thread {
    id: u64,
    graph: String,
    active: Vec<Active>,
    /// The RUN GRAPH node waiting for this thread: thread id and node.
    parent: Option<(u64, NodeId)>,
}

#[derive(Debug, Clone, Copy)]
struct Active {
    node: NodeId,
    /// Seconds since it became active.
    time: f32,
    /// RUN GRAPH: the thread it started.
    child: Option<u64>,
}

struct Showing {
    thread: u64,
    node: NodeId,
    serial: u64,
}

/// What a saved game keeps of the graphs.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RunnerState {
    pub flags: Flags,
    /// Characters renamed by graphs: id → name.
    pub names: BTreeMap<String, String>,
    pub threads: Vec<ThreadState>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThreadState {
    pub id: u64,
    pub graph: String,
    /// Active nodes: id, seconds active, the thread a RUN GRAPH started.
    pub active: Vec<(NodeId, f32, Option<u64>)>,
    #[serde(default)]
    pub parent: Option<(u64, NodeId)>,
}

/// Steps a frame may take passing through instant nodes before the rest
/// waits for the next frame (a loop of instant nodes would never end).
const STEP_BUDGET: usize = 10_000;

pub struct GraphRunner {
    /// The project folder graphs are loaded from.
    dir: PathBuf,
    graphs: HashMap<String, Arc<Graph>>,
    threads: Vec<Thread>,
    next_thread: u64,
    pub flags: Flags,
    /// Characters renamed by graphs: id → name.
    pub names: BTreeMap<String, String>,
    showing: Option<Showing>,
    serial: u64,
    commands: Vec<Command>,
    /// Nodes to enter: thread id and node.
    queue: Vec<(u64, NodeId)>,
    /// What happened, for the test drawer's events (the game takes them).
    pub log: Vec<String>,
}

impl GraphRunner {
    pub fn new(dir: PathBuf, flags: Flags) -> Self {
        Self {
            dir,
            graphs: HashMap::new(),
            threads: Vec::new(),
            next_thread: 1,
            flags,
            names: BTreeMap::new(),
            showing: None,
            serial: 0,
            commands: Vec::new(),
            queue: Vec::new(),
            log: Vec::new(),
        }
    }

    /// Uses `graph` for `path` instead of reading the file.
    pub fn insert_graph(&mut self, path: &str, graph: Graph) {
        self.graphs.insert(path.to_owned(), Arc::new(graph));
    }

    fn graph(&mut self, path: &str) -> Option<Arc<Graph>> {
        if let Some(graph) = self.graphs.get(path) {
            return Some(graph.clone());
        }
        match Graph::load(&self.dir.join(path)) {
            Ok(graph) => {
                let graph = Arc::new(graph);
                self.graphs.insert(path.to_owned(), graph.clone());
                Some(graph)
            }
            Err(err) => {
                log::error!("{err}");
                None
            }
        }
    }

    /// Starts a graph from its START nodes, unless it is running already.
    /// Returns whether it started.
    pub fn start(&mut self, path: &str) -> bool {
        if path.is_empty() || self.is_running(path) {
            return false;
        }
        self.spawn(path, None).is_some()
    }

    fn spawn(&mut self, path: &str, parent: Option<(u64, NodeId)>) -> Option<u64> {
        let graph = self.graph(path)?;
        let id = self.next_thread;
        self.next_thread += 1;
        self.threads.push(Thread {
            id,
            graph: path.to_owned(),
            active: Vec::new(),
            parent,
        });
        for start in graph.starts() {
            self.queue.push((id, start));
        }
        Some(id)
    }

    pub fn is_running(&self, path: &str) -> bool {
        self.threads.iter().any(|t| t.graph == path)
    }

    /// The running graphs and their active nodes, for the debug view.
    pub fn running(&self) -> Vec<(String, Arc<Graph>, Vec<NodeId>)> {
        self.threads
            .iter()
            .filter_map(|t| {
                let graph = self.graphs.get(&t.graph)?.clone();
                Some((
                    t.graph.clone(),
                    graph,
                    t.active.iter().map(|a| a.node).collect(),
                ))
            })
            .collect()
    }

    /// Moves the graphs on; returns what the world should do.
    pub fn update(&mut self, world: &World, dt: f32) -> Vec<Command> {
        for thread in &mut self.threads {
            for active in &mut thread.active {
                active.time += dt;
            }
        }
        let mut budget = STEP_BUDGET;
        // Instant chains settle within the frame: what fires can make
        // other waits fire too.
        for _ in 0..8 {
            self.run_queue(&mut budget);
            if !self.fire_waits(world) {
                break;
            }
        }
        self.run_queue(&mut budget);
        self.end_threads();
        self.pick_line();
        std::mem::take(&mut self.commands)
    }

    /// Waits whose moment has come pass on. Returns whether any did.
    fn fire_waits(&mut self, world: &World) -> bool {
        let running: Vec<u64> = self.threads.iter().map(|t| t.id).collect();
        let mut fired = Vec::new();
        for thread in &self.threads {
            let Some(graph) = self.graphs.get(&thread.graph) else {
                continue;
            };
            for active in &thread.active {
                let Some(node) = graph.node(active.node) else {
                    continue;
                };
                let ready = match &node.kind {
                    NodeKind::WhenInScene { scene } => same_path(&scene.0, world.scene),
                    NodeKind::WhenInZone { zone } => world.zone == Some(zone.0.as_str()),
                    NodeKind::WhenInTrigger { trigger } => world.triggers.contains(&trigger.0),
                    NodeKind::WhenOutOfTrigger { trigger } => !world.triggers.contains(&trigger.0),
                    NodeKind::WhenUsed { object } => world.used.contains(&object.0),
                    NodeKind::WhenPickedUp { object } => world.picked.contains(&object.0),
                    NodeKind::WaitUntil { condition } => condition.holds(&self.flags),
                    NodeKind::Wait { seconds } => active.time >= *seconds,
                    NodeKind::RunGraph { .. } => active.child.is_none_or(|c| !running.contains(&c)),
                    _ => false,
                };
                if ready {
                    fired.push((thread.id, active.node));
                }
            }
        }
        for &(thread, node) in &fired {
            if let Some(n) = self
                .thread_graph(thread)
                .and_then(|g| g.node(node).cloned())
                && !matches!(n.kind, NodeKind::RunGraph { .. })
            {
                self.log
                    .push(format!("{} {}", n.kind.title(), n.kind.summary().join(" ")));
            }
            self.leave(thread, node);
            self.pass(thread, node, 0);
        }
        !fired.is_empty()
    }

    /// Enters the queued nodes: actions act and pass on, waits become
    /// active.
    fn run_queue(&mut self, budget: &mut usize) {
        while let Some((thread, id)) = self.queue.pop() {
            if *budget == 0 {
                log::warn!("a graph loops through instant nodes; the rest waits a frame");
                self.queue.clear();
                return;
            }
            *budget -= 1;
            let Some(graph) = self.thread_graph(thread) else {
                continue;
            };
            let Some(node) = graph.node(id) else {
                continue;
            };
            let acts = !matches!(
                node.kind,
                NodeKind::Note { .. }
                    | NodeKind::Start
                    | NodeKind::Reply { .. }
                    | NodeKind::Branch { .. }
            ) && node.kind.category() == super::Category::Action
                || matches!(
                    node.kind,
                    NodeKind::Rename { .. } | NodeKind::RunGraph { .. }
                );
            if acts {
                self.log.push(format!(
                    "{} {}",
                    node.kind.title(),
                    node.kind.summary().join(" ")
                ));
            }
            match &node.kind {
                NodeKind::Note { .. } => {}
                NodeKind::Start | NodeKind::Reply { .. } => self.pass(thread, id, 0),
                NodeKind::Branch { condition } => {
                    let output = if condition.holds(&self.flags) { 0 } else { 1 };
                    self.pass(thread, id, output);
                }
                NodeKind::SetFlag { flag, value } => {
                    self.flags.set(&flag.0, value);
                    self.pass(thread, id, 0);
                }
                NodeKind::AddToFlag { flag, amount } => {
                    self.flags.add(&flag.0, *amount);
                    self.pass(thread, id, 0);
                }
                NodeKind::Door { door, action } => {
                    self.commands.push(Command::Door {
                        door: door.0.clone(),
                        action: *action,
                    });
                    self.pass(thread, id, 0);
                }
                NodeKind::Show { object, visible } => {
                    self.commands.push(Command::Show {
                        object: object.0.clone(),
                        visible: *visible,
                    });
                    self.pass(thread, id, 0);
                }
                NodeKind::PlaySound { sound, at, volume } => {
                    self.commands.push(Command::PlaySound {
                        sound: sound.0.clone(),
                        at: at.0.clone(),
                        volume: *volume,
                    });
                    self.pass(thread, id, 0);
                }
                NodeKind::LoadScene { scene } => {
                    self.commands.push(Command::LoadScene(scene.0.clone()));
                    self.pass(thread, id, 0);
                }
                NodeKind::SaveGame => {
                    self.commands.push(Command::Save);
                    self.pass(thread, id, 0);
                }
                NodeKind::Rename { character, name } => {
                    self.names.insert(character.0.clone(), name.clone());
                    self.pass(thread, id, 0);
                }
                NodeKind::RunGraph { graph: path } => {
                    if self.is_active(thread, id) {
                        continue;
                    }
                    match self.spawn(&path.0, Some((thread, id))) {
                        Some(child) => self.activate(thread, id, Some(child)),
                        None => self.pass(thread, id, 0),
                    }
                }
                // Events, waits and lines: active until their moment.
                _ => {
                    if !self.is_active(thread, id) {
                        self.activate(thread, id, None);
                    }
                }
            }
        }
    }

    fn thread_graph(&self, thread: u64) -> Option<Arc<Graph>> {
        let thread = self.threads.iter().find(|t| t.id == thread)?;
        self.graphs.get(&thread.graph).cloned()
    }

    fn is_active(&self, thread: u64, node: NodeId) -> bool {
        self.threads
            .iter()
            .find(|t| t.id == thread)
            .is_some_and(|t| t.active.iter().any(|a| a.node == node))
    }

    fn activate(&mut self, thread: u64, node: NodeId, child: Option<u64>) {
        if let Some(t) = self.threads.iter_mut().find(|t| t.id == thread) {
            t.active.push(Active {
                node,
                time: 0.0,
                child,
            });
        }
    }

    fn leave(&mut self, thread: u64, node: NodeId) {
        if let Some(t) = self.threads.iter_mut().find(|t| t.id == thread) {
            t.active.retain(|a| a.node != node);
        }
    }

    /// Queues everything `node`'s output leads to.
    fn pass(&mut self, thread: u64, node: NodeId, output: u32) {
        if let Some(graph) = self.thread_graph(thread) {
            // Popped from the back: push in reverse so the first connected
            // runs first.
            let targets: Vec<NodeId> = graph.targets(node, output).collect();
            for target in targets.into_iter().rev() {
                self.queue.push((thread, target));
            }
        }
    }

    /// Threads with nothing active have ended; RUN GRAPH nodes waiting on
    /// them go on next frame.
    fn end_threads(&mut self) {
        let queued: Vec<u64> = self.queue.iter().map(|(t, _)| *t).collect();
        self.threads
            .retain(|t| !t.active.is_empty() || queued.contains(&t.id));
    }

    /// Shows the first active line if none is on screen.
    fn pick_line(&mut self) {
        if let Some(showing) = &self.showing
            && self.is_active(showing.thread, showing.node)
        {
            return;
        }
        self.showing = None;
        let next = self.threads.iter().find_map(|t| {
            let graph = self.graphs.get(&t.graph)?;
            t.active
                .iter()
                .find_map(|a| match &graph.node(a.node)?.kind {
                    NodeKind::Line { speaker, sound, .. } => {
                        Some((t.id, a.node, speaker.0.clone(), sound.0.clone()))
                    }
                    _ => None,
                })
        });
        if let Some((thread, node, speaker, sound)) = next {
            self.serial += 1;
            self.showing = Some(Showing {
                thread,
                node,
                serial: self.serial,
            });
            self.log.push(format!("LINE {speaker}"));
            if !sound.is_empty() {
                self.commands.push(Command::LineSound { sound, speaker });
            }
        }
    }

    /// The line on screen, if any.
    pub fn line(&self) -> Option<LineView> {
        let showing = self.showing.as_ref()?;
        let graph = self.thread_graph(showing.thread)?;
        let NodeKind::Line { speaker, text, .. } = &graph.node(showing.node)?.kind else {
            return None;
        };
        let replies = self
            .replies(&graph, showing.node)
            .into_iter()
            .filter_map(|id| match &graph.node(id)?.kind {
                NodeKind::Reply { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect();
        Some(LineView {
            serial: showing.serial,
            speaker: speaker.0.clone(),
            text: text.clone(),
            replies,
        })
    }

    /// The replies on offer after a line: connected, and their condition
    /// holds.
    fn replies(&self, graph: &Graph, line: NodeId) -> Vec<NodeId> {
        graph
            .targets(line, 0)
            .filter(|&id| {
                matches!(graph.node(id).map(|n| &n.kind),
                    Some(NodeKind::Reply { condition, .. }) if condition.holds(&self.flags))
            })
            .collect()
    }

    /// Leaves the line on screen: with `reply`, through that reply.
    /// Ignored if the line offers replies and none (or a wrong one) is
    /// given.
    pub fn answer(&mut self, reply: Option<usize>) {
        let Some(showing) = self.showing.take() else {
            return;
        };
        let Some(graph) = self.thread_graph(showing.thread) else {
            return;
        };
        let replies = self.replies(&graph, showing.node);
        let chosen = match reply {
            Some(i) if i < replies.len() => Some(replies[i]),
            _ if replies.is_empty() => None,
            _ => {
                self.showing = Some(showing);
                return;
            }
        };
        self.leave(showing.thread, showing.node);
        // Everything that isn't a reply runs, plus the chosen reply.
        let others: Vec<NodeId> = graph
            .targets(showing.node, 0)
            .filter(|&id| {
                !matches!(
                    graph.node(id).map(|n| &n.kind),
                    Some(NodeKind::Reply { .. })
                )
            })
            .chain(chosen)
            .collect();
        for target in others.into_iter().rev() {
            self.queue.push((showing.thread, target));
        }
        let mut budget = STEP_BUDGET;
        self.run_queue(&mut budget);
        self.end_threads();
        self.pick_line();
    }

    /// Takes a graph out of play (with the graphs it started), keeping
    /// where it was; [`resume`](Self::resume) puts it back. A scene's graph
    /// pauses like this while the player is elsewhere.
    pub fn suspend(&mut self, path: &str) -> Vec<ThreadState> {
        let mut ids: Vec<u64> = self
            .threads
            .iter()
            .filter(|t| t.graph == path)
            .map(|t| t.id)
            .collect();
        loop {
            let more: Vec<u64> = self
                .threads
                .iter()
                .filter(|t| !ids.contains(&t.id) && t.parent.is_some_and(|(p, _)| ids.contains(&p)))
                .map(|t| t.id)
                .collect();
            if more.is_empty() {
                break;
            }
            ids.extend(more);
        }
        if self
            .showing
            .as_ref()
            .is_some_and(|s| ids.contains(&s.thread))
        {
            self.showing = None;
        }
        self.queue.retain(|(t, _)| !ids.contains(t));
        let state = self.state();
        self.threads.retain(|t| !ids.contains(&t.id));
        state
            .threads
            .into_iter()
            .filter(|t| ids.contains(&t.id))
            .collect()
    }

    /// Puts suspended graphs back in play where they were.
    pub fn resume(&mut self, threads: Vec<ThreadState>) {
        for t in threads {
            let Some(graph) = self.graph(&t.graph) else {
                continue;
            };
            self.next_thread = self.next_thread.max(t.id + 1);
            self.threads.push(Thread {
                id: t.id,
                graph: t.graph,
                active: t
                    .active
                    .into_iter()
                    .filter(|(node, _, _)| graph.node(*node).is_some())
                    .map(|(node, time, child)| Active { node, time, child })
                    .collect(),
                parent: t.parent,
            });
        }
    }

    /// Everything a saved game needs to carry on from here.
    pub fn state(&self) -> RunnerState {
        RunnerState {
            flags: self.flags.clone(),
            names: self.names.clone(),
            threads: self
                .threads
                .iter()
                .map(|t| ThreadState {
                    id: t.id,
                    graph: t.graph.clone(),
                    active: t.active.iter().map(|a| (a.node, a.time, a.child)).collect(),
                    parent: t.parent,
                })
                .collect(),
        }
    }

    /// Carries on from a saved state. Graphs or nodes that no longer exist
    /// are dropped.
    pub fn restore(&mut self, state: RunnerState) {
        self.flags = state.flags;
        self.names = state.names;
        self.threads.clear();
        self.queue.clear();
        self.showing = None;
        for t in state.threads {
            let Some(graph) = self.graph(&t.graph) else {
                continue;
            };
            self.next_thread = self.next_thread.max(t.id + 1);
            self.threads.push(Thread {
                id: t.id,
                graph: t.graph,
                active: t
                    .active
                    .into_iter()
                    .filter(|(node, _, _)| graph.node(*node).is_some())
                    .map(|(node, time, child)| Active { node, time, child })
                    .collect(),
                parent: t.parent,
            });
        }
        self.pick_line();
    }
}

/// Paths compared the way graphs write them (`/`, no leading `./`).
fn same_path(a: &str, b: &str) -> bool {
    let clean = |s: &str| s.replace('\\', "/").trim_start_matches("./").to_owned();
    !a.is_empty() && clean(a) == clean(b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::flags::{Condition, Test};
    use crate::scene::{CharacterRef, FlagRef, GraphRef, ObjectRef, SoundRef, ZoneRef};
    use glam::Vec2;

    /// A graph from a list of nodes; `links` as (from index, output, to
    /// index), indices into `kinds` (ids are index + 1).
    fn graph(kinds: Vec<NodeKind>, links: &[(usize, u32, usize)]) -> Graph {
        let mut g = Graph::default();
        for kind in kinds {
            g.add(kind, Vec2::ZERO);
        }
        for &(from, output, to) in links {
            assert!(
                g.connect(NodeId(from as u32 + 1), output, NodeId(to as u32 + 1)),
                "link {from} -> {to}"
            );
        }
        g
    }

    fn runner(graphs: Vec<(&str, Graph)>) -> GraphRunner {
        let mut runner = GraphRunner::new(PathBuf::new(), Flags::default());
        for (path, graph) in graphs {
            runner.insert_graph(path, graph);
        }
        runner
    }

    fn flag(name: &str) -> FlagRef {
        FlagRef(name.into())
    }

    #[test]
    fn a_door_opens_when_the_player_reaches_the_zone_after_the_switch() {
        let flow = graph(
            vec![
                NodeKind::Start,
                NodeKind::WhenUsed {
                    object: ObjectRef("switch".into()),
                },
                NodeKind::SetFlag {
                    flag: flag("lever"),
                    value: "true".into(),
                },
                NodeKind::WhenInZone {
                    zone: ZoneRef("hall".into()),
                },
                NodeKind::Branch {
                    condition: Condition {
                        flag: flag("lever"),
                        test: Test::IsTrue,
                        value: String::new(),
                    },
                },
                NodeKind::Door {
                    door: ObjectRef("gate".into()),
                    action: DoorAction::Open,
                },
            ],
            &[(0, 0, 1), (1, 0, 2), (0, 0, 3), (3, 0, 4), (4, 0, 5)],
        );
        let mut runner = runner(vec![("flow", flow)]);
        assert!(runner.start("flow"));
        assert!(!runner.start("flow"), "already running");
        assert!(runner.update(&World::default(), 0.1).is_empty());

        // In the hall without the switch: the branch says NO, nothing opens.
        let hall = World {
            zone: Some("hall"),
            ..Default::default()
        };
        assert!(runner.update(&hall, 0.1).is_empty());

        // The switch sets the flag; the hall wait has passed already.
        let used = ["switch".to_string()];
        let using = World {
            used: &used,
            ..Default::default()
        };
        assert!(runner.update(&using, 0.1).is_empty());
        assert!(runner.flags.is_true("lever"));
        assert!(!runner.is_running("flow"), "everything has passed");
    }

    #[test]
    fn waits_count_seconds_and_conditions_wait_for_flags() {
        let flow = graph(
            vec![
                NodeKind::Start,
                NodeKind::Wait { seconds: 1.0 },
                NodeKind::SetFlag {
                    flag: flag("a"),
                    value: "true".into(),
                },
                NodeKind::WaitUntil {
                    condition: Condition {
                        flag: flag("a"),
                        ..Default::default()
                    },
                },
                NodeKind::Show {
                    object: ObjectRef("lamp".into()),
                    visible: true,
                },
            ],
            &[(0, 0, 1), (1, 0, 2), (0, 0, 3), (3, 0, 4)],
        );
        let mut runner = runner(vec![("flow", flow)]);
        runner.start("flow");
        assert!(
            runner.update(&World::default(), 0.1).is_empty(),
            "starts waiting"
        );
        assert!(runner.update(&World::default(), 0.5).is_empty());
        // The wait ends, the flag is set and the other thread's wait sees
        // it in the same frame.
        assert_eq!(
            runner.update(&World::default(), 0.6),
            vec![Command::Show {
                object: "lamp".into(),
                visible: true
            }]
        );
    }

    /// keeper says a line; replies "yes" (sets kind) / "no"; then a line
    /// that only goes on.
    fn dialogue() -> Graph {
        graph(
            vec![
                NodeKind::Start,
                NodeKind::Line {
                    speaker: CharacterRef("keeper".into()),
                    text: "Will you help?".into(),
                    sound: SoundRef("audio/hm.wav".into()),
                },
                NodeKind::Reply {
                    text: "Yes".into(),
                    condition: Condition::default(),
                },
                NodeKind::Reply {
                    text: "Only with the key".into(),
                    condition: Condition {
                        flag: flag("key"),
                        ..Default::default()
                    },
                },
                NodeKind::Reply {
                    text: "No".into(),
                    condition: Condition::default(),
                },
                NodeKind::SetFlag {
                    flag: flag("kind"),
                    value: "true".into(),
                },
                NodeKind::Line {
                    speaker: CharacterRef("keeper".into()),
                    text: "Thank you.".into(),
                    sound: SoundRef::default(),
                },
                NodeKind::Rename {
                    character: CharacterRef("keeper".into()),
                    name: "Ada".into(),
                },
            ],
            &[
                (0, 0, 1),
                (1, 0, 2),
                (1, 0, 3),
                (1, 0, 4),
                (2, 0, 5),
                (5, 0, 6),
                (6, 0, 7),
            ],
        )
    }

    #[test]
    fn dialogue_offers_replies_and_follows_the_one_picked() {
        let flow = graph(
            vec![
                NodeKind::Start,
                NodeKind::RunGraph {
                    graph: GraphRef("talk".into()),
                },
                NodeKind::Door {
                    door: ObjectRef("gate".into()),
                    action: DoorAction::Toggle,
                },
            ],
            &[(0, 0, 1), (1, 0, 2)],
        );
        let mut runner = runner(vec![("flow", flow), ("talk", dialogue())]);
        runner.start("flow");
        let commands = runner.update(&World::default(), 0.1);
        assert_eq!(
            commands,
            vec![Command::LineSound {
                sound: "audio/hm.wav".into(),
                speaker: "keeper".into()
            }]
        );
        let line = runner.line().unwrap();
        assert_eq!(line.text, "Will you help?");
        assert_eq!(line.replies, vec!["Yes", "No"], "the key reply is hidden");

        runner.answer(None); // a reply must be picked
        assert_eq!(runner.line().unwrap().serial, line.serial);
        runner.answer(Some(0));
        assert!(runner.flags.is_true("kind"));
        let thanks = runner.line().unwrap();
        assert_eq!(
            (thanks.text.as_str(), thanks.replies.len()),
            ("Thank you.", 0)
        );
        assert_ne!(thanks.serial, line.serial);

        runner.answer(None);
        assert_eq!(runner.line(), None);
        assert_eq!(runner.names.get("keeper").map(String::as_str), Some("Ada"));
        // The dialogue has ended: the flow goes on next frame.
        assert_eq!(
            runner.update(&World::default(), 0.1),
            vec![Command::Door {
                door: "gate".into(),
                action: DoorAction::Toggle
            }]
        );
        assert!(runner.running().is_empty());
    }

    #[test]
    fn a_saved_state_carries_on_where_it_was() {
        let mut first = runner(vec![("talk", dialogue())]);
        first.flags.set("key", "true");
        first.start("talk");
        first.update(&World::default(), 0.1);
        let state = first.state();

        let mut second = runner(vec![("talk", dialogue())]);
        second.restore(state);
        let line = second.line().unwrap();
        assert_eq!(line.replies.len(), 3, "the key is remembered");
        second.answer(Some(2));
        assert_eq!(second.line(), None, "\"No\" leads nowhere");
    }

    #[test]
    fn a_suspended_graph_carries_on_where_it_was() {
        let scene = graph(
            vec![
                NodeKind::Start,
                NodeKind::WhenUsed {
                    object: ObjectRef("lever".into()),
                },
                NodeKind::Show {
                    object: ObjectRef("lamp".into()),
                    visible: true,
                },
            ],
            &[(0, 0, 1), (1, 0, 2)],
        );
        let mut runner = runner(vec![("scene", scene)]);
        runner.start("scene");
        runner.update(&World::default(), 0.1);
        let paused = runner.suspend("scene");
        assert_eq!(paused.len(), 1);
        assert!(runner.running().is_empty());
        let lever = ["lever".to_string()];
        let using = World {
            used: &lever,
            ..Default::default()
        };
        assert!(
            runner.update(&using, 0.1).is_empty(),
            "paused graphs don't hear"
        );
        runner.resume(paused);
        assert_eq!(runner.update(&using, 0.1).len(), 1);
    }

    #[test]
    fn instant_loops_stop_instead_of_hanging() {
        let mut flow = graph(
            vec![
                NodeKind::Start,
                NodeKind::AddToFlag {
                    flag: flag("n"),
                    amount: 1.0,
                },
                NodeKind::AddToFlag {
                    flag: flag("n"),
                    amount: 1.0,
                },
            ],
            &[(0, 0, 1), (1, 0, 2)],
        );
        flow.connect(NodeId(3), 0, NodeId(2));
        let mut runner = runner(vec![("flow", flow)]);
        runner.start("flow");
        runner.update(&World::default(), 0.1);
        assert!(runner.flags.get("n").as_number() > 100.0);
    }

    #[test]
    fn the_sandbox_story_plays() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sandbox");
        let flags = crate::graph::flags::FlagDefs::load(&dir);
        assert!(flags.names().contains(&"double_jump".to_string()));
        let scene = crate::scene::Scene::load(&dir.join("scenes/test_level.ron")).unwrap();
        assert!(scene.find("doorstep").is_some() && scene.find("gate").is_some());

        let mut runner = GraphRunner::new(dir, flags.start());
        assert!(runner.start("graphs/main.ron"));
        let lever = ["lever".to_string()];
        let doorstep = ["doorstep".to_string()];
        let at_door = World {
            triggers: &doorstep,
            ..Default::default()
        };
        assert!(
            runner.update(&at_door, 0.1).is_empty(),
            "not before the lever"
        );
        runner.update(
            &World {
                used: &lever,
                ..Default::default()
            },
            0.1,
        );
        let commands = runner.update(&at_door, 0.1);
        assert!(commands.contains(&Command::Door {
            door: "gate".into(),
            action: DoorAction::Open
        }));
        assert!(
            commands
                .iter()
                .any(|c| matches!(c, Command::Show { object, .. } if object == "room lamp"))
        );

        // The keeper: unknown at first, Ada after a friendly answer.
        assert!(runner.start("graphs/keeper.ron"));
        runner.update(&World::default(), 0.1);
        assert_eq!(runner.line().unwrap().text, "Who goes there?");
        runner.answer(Some(0));
        runner.answer(None);
        assert_eq!(runner.names.get("keeper").map(String::as_str), Some("Ada"));
        assert!(runner.flags.is_true("met_keeper") && runner.flags.is_true("kind"));
    }
}
