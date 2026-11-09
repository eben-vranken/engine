//! Every editor action as a command, so the menus, the command palette
//! (CTRL+K) and the shortcut list offer the same things under the same
//! names, and run them the same way.

use super::file_dialogs::Purpose;
use super::gizmo::View;
use super::name_dialog::Naming;
use super::objects::primitives;
use super::project_page::Section;
use super::{Editor, EditorOutput};
use crate::player::settings::PlayerSettings;
use crate::scene::Scene;
use std::path::PathBuf;

/// The three places the editor works in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Workspace {
    /// The start screen and the project's settings.
    Project,
    /// Scenes.
    Build,
    /// Graphs and flags.
    Logic,
}

/// The transform gizmo's tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GizmoTool {
    Move,
    Rotate,
    Scale,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum Command {
    SaveAll,
    Deselect,
    Gizmo(GizmoTool),
    NewProject,
    OpenProject,
    OpenPackage,
    ExportPackage,
    NewScene,
    NewGraph,
    NewScript,
    Save,
    SaveSceneAs,
    Settings(Section),
    Quit,
    Undo,
    Redo,
    Duplicate,
    Delete,
    Search,
    Play,
    PlayFromSave,
    PlayInWindow,
    BakeLighting,
    BakeAcoustics,
    /// An index into `primitives()`.
    AddPrimitive(usize),
    AddPlayerStart,
    AddZoneVolume,
    AddPortal,
    AddZone,
    Go(Workspace),
    ToggleLeft,
    ToggleRight,
    ToggleZones,
    ToggleTriggers,
    ToggleProbes,
    ToggleHints,
    ToggleStats,
    Shortcuts,
    OpenScene(PathBuf),
    OpenGraph(PathBuf),
    Select(usize),
    /// An index into `new_components()`, for the selected object.
    AddBehavior(usize),
    /// An index into `NodeKind::all()`, for the open graph.
    AddNode(usize),
}

impl Command {
    pub fn label(&self) -> String {
        use Command::*;
        match self {
            NewProject => "NEW PROJECT…".into(),
            OpenProject => "OPEN PROJECT…".into(),
            OpenPackage => "OPEN PACKAGE…".into(),
            ExportPackage => "EXPORT PACKAGE…".into(),
            NewScene => "NEW SCENE…".into(),
            NewGraph => "NEW GRAPH…".into(),
            NewScript => "NEW SCRIPT…".into(),
            Save => "SAVE".into(),
            SaveAll => "SAVE ALL".into(),
            Deselect => "DESELECT".into(),
            Gizmo(GizmoTool::Move) => "MOVE TOOL".into(),
            Gizmo(GizmoTool::Rotate) => "ROTATE TOOL".into(),
            Gizmo(GizmoTool::Scale) => "SCALE TOOL".into(),
            SaveSceneAs => "SAVE SCENE AS…".into(),
            Settings(section) => format!("SETTINGS › {}", section.title()),
            Quit => "QUIT".into(),
            Undo => "UNDO".into(),
            Redo => "REDO".into(),
            Duplicate => "DUPLICATE".into(),
            Delete => "DELETE".into(),
            Search => "SEARCH OR RUN…".into(),
            Play => "PLAY FROM START".into(),
            PlayFromSave => "PLAY FROM SAVE".into(),
            PlayInWindow => "PLAY IN A WINDOW, AS A PLAYER".into(),
            BakeLighting => "BAKE LIGHTING".into(),
            BakeAcoustics => "BAKE ACOUSTICS".into(),
            AddPrimitive(i) => format!(
                "ADD {}",
                primitives()
                    .get(*i)
                    .map_or("?".into(), |(k, _)| k.to_uppercase())
            ),
            AddPlayerStart => "ADD PLAYER START".into(),
            AddZoneVolume => "ADD ZONE VOLUME".into(),
            AddPortal => "ADD PORTAL".into(),
            AddZone => "ADD ZONE".into(),
            Go(Workspace::Project) => "GO TO PROJECT".into(),
            Go(Workspace::Build) => "GO TO BUILD".into(),
            Go(Workspace::Logic) => "GO TO LOGIC".into(),
            ToggleLeft => "SHOW / HIDE LEFT PANEL".into(),
            ToggleRight => "SHOW / HIDE INSPECTOR".into(),
            ToggleZones => "SHOW / HIDE ZONE MARKERS".into(),
            ToggleTriggers => "SHOW / HIDE TRIGGER BOXES".into(),
            ToggleProbes => "SHOW / HIDE REVERB PROBES".into(),
            ToggleHints => "SHOW / HIDE KEY HINTS".into(),
            ToggleStats => "SHOW / HIDE STATS".into(),
            Shortcuts => "ALL SHORTCUTS".into(),
            OpenScene(p) => format!("OPEN {}", p.display()).to_uppercase(),
            OpenGraph(p) => format!("OPEN {}", p.display()).to_uppercase(),
            Select(_) => "SELECT".into(),
            AddBehavior(i) => format!(
                "ADD BEHAVIOR › {}",
                super::object_panel::new_components()
                    .get(*i)
                    .map_or("?".into(), |(n, _)| n.to_uppercase())
            ),
            AddNode(i) => format!(
                "ADD NODE › {}",
                crate::graph::NodeKind::all()
                    .get(*i)
                    .map_or("?", |k| k.title())
            ),
        }
    }

    /// Its keys, from the binding table.
    pub fn shortcut(&self) -> &'static str {
        match self {
            Command::Play => "F5",
            Command::PlayInWindow => "SHIFT+F5",
            Command::Shortcuts => "?",
            _ => super::keys::keys_of(self),
        }
    }

    /// The actions offered everywhere (not objects, files or nodes).
    pub fn all() -> Vec<Command> {
        use Command::*;
        let mut list = vec![
            Search,
            Save,
            SaveAll,
            Undo,
            Redo,
            Duplicate,
            Delete,
            Play,
            PlayFromSave,
            PlayInWindow,
            Go(Workspace::Project),
            Go(Workspace::Build),
            Go(Workspace::Logic),
            NewScene,
            NewGraph,
            NewScript,
            SaveSceneAs,
            NewProject,
            OpenProject,
            OpenPackage,
            ExportPackage,
            BakeLighting,
            BakeAcoustics,
        ];
        list.extend((0..primitives().len()).map(AddPrimitive));
        list.extend([AddPlayerStart, AddZoneVolume, AddPortal, AddZone]);
        list.extend(Section::ALL.iter().map(|s| Settings(*s)));
        list.extend([
            ToggleLeft,
            ToggleRight,
            ToggleZones,
            ToggleTriggers,
            ToggleProbes,
            ToggleHints,
            ToggleStats,
            Shortcuts,
            Quit,
        ]);
        list
    }
}

impl Editor {
    /// Whether `command` can run now.
    pub(super) fn can_run(&self, command: &Command, scene: &Scene) -> bool {
        use Command::*;
        let build = self.workspace() == Workspace::Build;
        let waiting = self.file_dialog_open();
        match command {
            _ if self.playing => matches!(command, Search | Shortcuts | ToggleHints | ToggleStats),
            // A script's text edits undo inside the code editor.
            Undo | Redo | Duplicate | Delete if self.script.is_some() => false,
            NewProject | OpenProject | OpenPackage | ExportPackage => !waiting,
            SaveSceneAs => build && !waiting,
            Undo => match &self.graph {
                Some(doc) => doc.can_undo(),
                None => self.history.undo_len() > 0,
            },
            Redo => match &self.graph {
                Some(doc) => doc.can_redo(),
                None => self.history.redo_len() > 0,
            },
            Duplicate | Delete => match &self.graph {
                Some(doc) => doc.selected.is_some(),
                None => build && self.selected.is_some_and(|i| i < scene.objects.len()),
            },
            Play => build,
            PlayFromSave => build && crate::save::exists(&self.project_dir),
            BakeLighting => build && !self.light_status.starts_with("BAKING"),
            BakeAcoustics => build && !self.bake_status.starts_with("BAKING"),
            AddPrimitive(_) | AddPlayerStart | AddZoneVolume | AddPortal => {
                build && !scene.zones.is_empty()
            }
            AddZone | ToggleZones | ToggleTriggers | ToggleProbes => build,
            Deselect | Gizmo(_) => build,
            AddBehavior(_) => build && self.selected.is_some(),
            AddNode(_) => self.graph.is_some(),
            _ => true,
        }
    }

    /// Runs a command (if it can run now).
    pub(super) fn run(
        &mut self,
        command: Command,
        scene: &mut Scene,
        player: &mut PlayerSettings,
        view: &View,
        output: &mut EditorOutput,
    ) {
        use Command::*;
        if !self.can_run(&command, scene) {
            return;
        }
        let mut change = None;
        match command {
            NewProject => self.ask_file(Purpose::NewProject),
            OpenProject => self.ask_file(Purpose::OpenProject),
            OpenPackage => self.ask_file(Purpose::OpenPackage),
            ExportPackage => self.ask_file(Purpose::ExportPackage),
            SaveSceneAs => self.ask_file(Purpose::SaveSceneAs),
            NewScene => self.ask_name(Naming::Scene),
            NewGraph => self.ask_name(Naming::Graph),
            NewScript => self.ask_name(Naming::Script),
            Save => output.settled |= self.save(scene, player),
            SaveAll => {
                let mut settled = false;
                self.save_all(scene, player, &mut settled);
                output.settled |= settled;
            }
            Deselect => output.settled |= self.select(scene, None),
            Gizmo(tool) => {
                use transform_gizmo::config::GizmoModeKind;
                self.gizmo.kind = match tool {
                    GizmoTool::Move => GizmoModeKind::Translate,
                    GizmoTool::Rotate => GizmoModeKind::Rotate,
                    GizmoTool::Scale => GizmoModeKind::Scale,
                };
            }
            Settings(section) => self.open_project_page(section),
            Quit => {
                if self.request_close() {
                    output.exit = true;
                }
            }
            Undo => change = self.undo(scene, player),
            Redo => change = self.redo(scene, player),
            Duplicate => change = self.duplicate_selected(scene),
            Delete => change = self.delete_selected(scene),
            Search => self.open_palette(None),
            Play => output.toggle_play = true,
            PlayFromSave => output.resume_play = true,
            PlayInWindow => output.play_window = true,
            BakeLighting => output.bake_lighting = true,
            BakeAcoustics => output.bake = true,
            AddPrimitive(i) => {
                if let Some((kind, shape)) = primitives().into_iter().nth(i) {
                    change = self.add_primitive(scene, kind, shape, view);
                }
            }
            AddPlayerStart => change = self.add_player_start(scene, view),
            AddZoneVolume => change = self.add_zone_volume(scene, view),
            AddPortal => change = self.add_portal(scene, view),
            AddZone => change = Some(self.add_zone(scene)),
            Go(workspace) => self.go(workspace, output),
            ToggleLeft => self.show_scene = !self.show_scene,
            ToggleRight => self.show_inspector = !self.show_inspector,
            ToggleZones => self.show_zones = !self.show_zones,
            ToggleTriggers => self.show_triggers = !self.show_triggers,
            ToggleProbes => self.show_probes = !self.show_probes,
            ToggleHints => self.show_hints = !self.show_hints,
            ToggleStats => self.show_stats = !self.show_stats,
            Shortcuts => self.shortcuts_open = true,
            OpenScene(path) => match self.tab_of(&path) {
                Some(index) => output.switch_tab = Some(index),
                None => output.new_tab = Some(path),
            },
            OpenGraph(path) => output.open_graph = Some(path),
            Select(index) => {
                output.settled |= self.select(scene, Some(index));
                self.reveal_selection = true;
            }
            AddBehavior(i) => {
                if let (Some(index), Some((_, component))) = (
                    self.selected.filter(|&s| s < scene.objects.len()),
                    super::object_panel::new_components().into_iter().nth(i),
                ) {
                    let before = scene.objects[index].clone();
                    let kind = std::mem::discriminant(&component);
                    let object = &mut scene.objects[index];
                    if !object
                        .components
                        .iter()
                        .any(|c| std::mem::discriminant(c) == kind)
                    {
                        object.components.push(component);
                        self.begin_edit(index, &before);
                        output.changed.push(super::history::Change::Modified(index));
                    }
                }
            }
            AddNode(i) => {
                if let (Some(doc), Some(kind)) = (
                    &mut self.graph,
                    crate::graph::NodeKind::all().into_iter().nth(i),
                ) {
                    doc.add_in_view(kind);
                }
            }
        }
        if let Some(change) = change {
            output.changed.push(change);
            output.settled = true;
        }
    }

    /// The workspace on screen.
    /// Whether the 3D view is on screen (BUILD), for what's drawn over it.
    pub fn shows_view(&self) -> bool {
        self.workspace() == Workspace::Build
    }

    pub fn workspace(&self) -> Workspace {
        if self.project_page.is_some() {
            Workspace::Project
        } else if self.graph.is_some() || self.script.is_some() {
            Workspace::Logic
        } else {
            Workspace::Build
        }
    }

    /// Switches workspace: BUILD and LOGIC go back to their last tab (or
    /// open one), PROJECT opens the settings page.
    pub(super) fn go(&mut self, workspace: Workspace, output: &mut EditorOutput) {
        match workspace {
            Workspace::Project => {
                if self.project_page.is_none() {
                    self.open_project_page(Section::Start);
                }
            }
            Workspace::Build | Workspace::Logic => {
                // Unsaved game.ron changes aren't dropped without asking.
                if self.project_unsaved() {
                    self.confirm = Some(super::confirm::Confirm::LeaveProject(workspace));
                    return;
                }
                self.project_page = None;
                let graphs = workspace == Workspace::Logic;
                if self.is_graph_tab(self.active_tab()) == graphs {
                    return;
                }
                if let Some(index) = self.last_tab(graphs) {
                    output.switch_tab = Some(index);
                } else if graphs {
                    self.index.refresh(&self.project_dir, false);
                    // The open scene's own graph first.
                    let scene = crate::graph::scene_graph(&crate::graph::key(&self.scene_path));
                    let first = Some(scene.clone())
                        .filter(|g| self.index.graphs.contains_key(g))
                        .or_else(|| self.index.graphs.keys().next().cloned());
                    match first {
                        Some(path) => output.open_graph = Some(PathBuf::from(path)),
                        None => self.ask_name(Naming::Graph),
                    }
                } else {
                    match scene_files_first(&self.project_dir) {
                        Some(path) => output.new_tab = Some(path),
                        None => self.ask_name(Naming::Scene),
                    }
                }
            }
        }
    }
}

fn scene_files_first(dir: &std::path::Path) -> Option<PathBuf> {
    let start = crate::project::Project::open(dir)
        .ok()
        .map(|p| p.settings.start_scene);
    start.or_else(|| super::files::scene_files(dir).into_iter().next())
}
