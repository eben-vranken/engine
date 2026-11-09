//! The running story: gameplay graphs connected to the world. Each frame
//! the graphs hear what the player did (zones, triggers, switches,
//! pickups) and what they ask for happens (doors, objects shown or hidden,
//! sounds, scene loads, saves). Also dialogue on screen, ability gates,
//! the zones visited for the map, and saved games.

use super::game::spawn_point;
use super::{CameraMode, Running};
use crate::behaviors::Behaviors;
use crate::dialogue::DialogueStyle;
use crate::editor::graph_canvas::CanvasView;
use crate::graph::flags::FlagDefs;
use crate::graph::run::{Command, GraphRunner, LineView, World};
use crate::graph::{DoorAction, key};
use crate::player::{Player, Spawn};
use crate::project::Project;
use crate::save::{self, SaveGame, SceneState, WorldState};
use crate::scene::{Component, Scene};
use crate::zones::ZoneMap;
use glam::Vec3;
use std::path::PathBuf;
use winit::keyboard::KeyCode;

pub(super) struct Story {
    pub runner: GraphRunner,
    pub world: WorldState,
    pub style: DialogueStyle,
    /// Whether the style's font loaded (`None`: not tried yet).
    pub custom_font: Option<bool>,
    /// The line on screen.
    pub line: Option<LineUi>,
    pub map_open: bool,
    /// F3: the running graphs and flags (for testing).
    pub debug_open: bool,
    pub debug_view: CanvasView,
    /// Which running graph the debug view shows.
    pub debug_graph: usize,
    /// A short message ("GAME SAVED") and the seconds it stays.
    pub toast: Option<(String, f32)>,
    /// Objects the player used and picked up this frame, by name.
    used: Vec<String>,
    picked: Vec<String>,
}

pub(super) struct LineUi {
    pub view: LineView,
    /// Seconds on screen (the text types out over them).
    pub time: f32,
    /// The highlighted reply.
    pub choice: usize,
}

impl Story {
    /// Dialogue or the map is open: the player stands still and can't use
    /// things.
    pub fn busy(&self) -> bool {
        self.line.is_some() || self.map_open
    }

    fn toast(&mut self, text: &str) {
        self.toast = Some((text.into(), 2.5));
    }
}

/// What the player can do with the object they look at.
enum Use {
    Switch,
    Pickup,
    Talk(String),
    Save,
}

impl Running {
    /// Starts the game's graphs, flags and world state; with `resume`, from
    /// the saved game if there is one.
    pub(super) fn start_story(&mut self, resume: bool) {
        let settings = Project::open(&self.game_dir).map(|p| p.settings).ok();
        let flags = FlagDefs::load(&self.game_dir).start();
        self.story = Some(Story {
            runner: GraphRunner::new(self.game_dir.clone(), flags),
            world: WorldState::default(),
            style: settings
                .as_ref()
                .map(|s| s.dialogue.clone())
                .unwrap_or_default(),
            custom_font: None,
            line: None,
            map_open: false,
            debug_open: false,
            debug_view: CanvasView::default(),
            debug_graph: 0,
            toast: None,
            used: Vec::new(),
            picked: Vec::new(),
        });
        self.apply_scene_state();
        if resume && let Some(saved) = save::load(&self.game_dir) {
            self.load_game(saved);
            return;
        }
        if let Some(flow) = settings.and_then(|s| s.flow)
            && let Some(story) = &mut self.story
            && !story.runner.start(&key(&flow))
        {
            log::error!("the flow graph {} didn't start", flow.display());
        }
    }

    /// The loaded scene as graphs and saves name it.
    pub(super) fn scene_key(&self) -> String {
        let path = match (&self.play_scene, &self.editor) {
            (Some(path), _) => path,
            (None, Some(editor)) => &editor.scene_path,
            (None, None) => &self.scene_path,
        };
        key(path)
    }

    /// What the player changed in this scene goes into the world state.
    fn record_scene_state(&mut self) {
        let key = self.scene_key();
        let Some(story) = &mut self.story else { return };
        let mut state = SceneState::default();
        for object in &self.scene.objects {
            let starts_hidden = object.components.contains(&Component::StartsHidden);
            if object.hidden || starts_hidden {
                state.hidden.insert(object.name.clone(), object.hidden);
            }
        }
        if let Some(behaviors) = &self.behaviors {
            for (index, open) in behaviors.doors() {
                state
                    .doors
                    .insert(self.scene.objects[index].name.clone(), open);
            }
        }
        story.world.scenes.insert(key, state);
    }

    /// The scene as the player left it: hidden objects, open doors, gates.
    fn apply_scene_state(&mut self) {
        let key = self.scene_key();
        let Some(story) = &self.story else { return };
        let state = story.world.scenes.get(&key);
        for object in &mut self.scene.objects {
            object.hidden = object.components.contains(&Component::StartsHidden);
            if let Some(&hidden) = state.and_then(|s| s.hidden.get(&object.name)) {
                object.hidden = hidden;
            }
        }
        if let (Some(state), Some(behaviors)) = (state, &mut self.behaviors) {
            for (name, &open) in &state.doors {
                if let Some(index) = self.scene.find(name) {
                    behaviors.snap_door(index, open);
                }
            }
        }
        self.update_gates();
        self.renderer.set_scene(&self.scene, &mut self.assets);
        self.rebuild_physics();
    }

    /// Gates whose flag is true are hidden. Returns whether any changed.
    fn update_gates(&mut self) -> bool {
        let Some(story) = &self.story else {
            return false;
        };
        let mut changed = false;
        for object in &mut self.scene.objects {
            for component in &object.components {
                if let Component::AbilityGate(gate) = component {
                    let open =
                        !gate.requires.0.is_empty() && story.runner.flags.is_true(&gate.requires.0);
                    if object.hidden != open {
                        object.hidden = open;
                        changed = true;
                    }
                }
            }
        }
        changed
    }

    /// Where the player's head is (sounds "at the player" come from here).
    fn listener(&self) -> Vec3 {
        match self.mode {
            CameraMode::Player => self.player.camera(&self.settings).position,
            CameraMode::Fly => self.fly.camera.position,
        }
    }

    /// Where the player's body is, for trigger zones.
    fn body(&self) -> Vec3 {
        match self.mode {
            CameraMode::Player => self.player.feet + Vec3::Y * 0.9,
            CameraMode::Fly => self.fly.camera.position,
        }
    }

    /// What the player looks at can be used: a prompt, and the use.
    fn usable(&self, index: usize) -> Option<(Use, String)> {
        let object = self.scene.objects.get(index)?;
        let story = self.story.as_ref();
        object.components.iter().find_map(|c| match c {
            Component::Switch(s) if !s.pressure_plate => Some((Use::Switch, "E  USE".into())),
            Component::Pickup(_) => Some((Use::Pickup, "E  TAKE".into())),
            Component::SavePoint => Some((Use::Save, "E  SAVE".into())),
            Component::Character(character) if !character.dialogue.0.is_empty() => {
                let name = story
                    .and_then(|s| s.runner.names.get(&character.id).cloned())
                    .unwrap_or_else(|| character.name.clone());
                let prompt = if name.is_empty() {
                    "E  TALK".into()
                } else {
                    format!("E  TALK TO {}", name.to_uppercase())
                };
                Some((Use::Talk(character.dialogue.0.clone()), prompt))
            }
            _ => None,
        })
    }

    /// The object in reach the player is looking at, if it can be used:
    /// shows its prompt, and acts on E. Returns the switch pressed, for
    /// behaviors.
    pub(super) fn use_objects(&mut self) -> Option<usize> {
        self.prompt = None;
        if self.mode != CameraMode::Player || self.story.as_ref().is_some_and(Story::busy) {
            return None;
        }
        let eye = self.player.camera(&self.settings);
        let (index, _) = self
            .physics
            .pick(eye.position, eye.forward(), super::game::REACH)?;
        let (action, prompt) = self.usable(index)?;
        self.prompt = Some(prompt);
        if !self.input.pressed(KeyCode::KeyE) {
            return None;
        }
        let name = self.scene.objects[index].name.clone();
        match action {
            Use::Switch => {
                if let Some(story) = &mut self.story {
                    story.used.push(name);
                }
                return Some(index);
            }
            Use::Pickup => self.pick_up(index),
            Use::Talk(graph) => {
                if let Some(story) = &mut self.story {
                    story.runner.start(&graph);
                }
            }
            Use::Save => self.save_game(),
        }
        None
    }

    fn pick_up(&mut self, index: usize) {
        let object = &mut self.scene.objects[index];
        object.hidden = true;
        let position = object.transform.position;
        let pickup = object.components.iter().find_map(|c| match c {
            Component::Pickup(p) => Some(p.clone()),
            _ => None,
        });
        let name = object.name.clone();
        if let Some(story) = &mut self.story {
            story.picked.push(name);
            if let Some(pickup) = &pickup {
                story.runner.flags.set(&pickup.flag.0, "true");
            }
        }
        if let (Some(pickup), Some(audio)) = (&pickup, &mut self.audio)
            && !pickup.sound.0.is_empty()
        {
            audio.play_sound(&pickup.sound.0, position, 1.0);
        }
        self.renderer.set_scene(&self.scene, &mut self.assets);
        self.rebuild_physics();
    }

    /// Moves the story on by a frame: dialogue keys, the graphs, what they
    /// ask for, gates and visited zones.
    pub(super) fn update_story(&mut self, dt: f32) {
        if self.story.is_none() {
            return;
        }
        self.dialogue_keys();
        let scene_key = self.scene_key();
        let body = self.body();
        let zone = ZoneMap::new(&self.scene)
            .zone_at(body)
            .map(|z| self.scene.zones[z].name.clone());
        let triggers: Vec<String> = self
            .scene
            .objects
            .iter()
            .filter(|o| {
                o.components.iter().any(|c| match c {
                    Component::TriggerZone(t) => t.contains(&o.transform, body),
                    _ => false,
                })
            })
            .map(|o| o.name.clone())
            .collect();
        let Some(story) = &mut self.story else { return };
        if let Some(zone) = &zone {
            story.world.visit(&scene_key, zone);
        }
        let used = std::mem::take(&mut story.used);
        let picked = std::mem::take(&mut story.picked);
        let world = World {
            scene: &scene_key,
            zone: zone.as_deref(),
            triggers: &triggers,
            used: &used,
            picked: &picked,
        };
        let commands = story.runner.update(&world, dt);
        if let Some((_, left)) = &mut story.toast {
            *left -= dt;
            if *left <= 0.0 {
                story.toast = None;
            }
        }
        // The line on screen follows the runner's.
        match (story.runner.line(), &mut story.line) {
            (Some(view), Some(line)) if line.view.serial == view.serial => {
                line.time += dt;
                line.choice = line.choice.min(view.replies.len().saturating_sub(1));
                line.view = view;
            }
            (Some(view), _) => {
                story.line = Some(LineUi {
                    view,
                    time: 0.0,
                    choice: 0,
                })
            }
            (None, _) => story.line = None,
        }
        self.run_commands(commands);
        if self.update_gates() {
            self.renderer.set_scene(&self.scene, &mut self.assets);
            self.rebuild_physics();
        }
    }

    /// Keys while a line is on screen: W/S (or arrows) pick a reply, 1–9
    /// pick directly, E / Space / Enter go on (or show all of the text
    /// first while it is still typing).
    fn dialogue_keys(&mut self) {
        let Some(story) = &mut self.story else { return };
        let Some(line) = &mut story.line else { return };
        let input = &self.input;
        let count = line.view.replies.len();
        if count > 0 {
            if input.pressed(KeyCode::KeyS) || input.pressed(KeyCode::ArrowDown) {
                line.choice = (line.choice + 1) % count;
            }
            if input.pressed(KeyCode::KeyW) || input.pressed(KeyCode::ArrowUp) {
                line.choice = (line.choice + count - 1) % count;
            }
        }
        let (_, complete) =
            crate::dialogue::typed(&line.view.text, line.time, story.style.typing_speed);
        let digits = [
            KeyCode::Digit1,
            KeyCode::Digit2,
            KeyCode::Digit3,
            KeyCode::Digit4,
            KeyCode::Digit5,
            KeyCode::Digit6,
            KeyCode::Digit7,
            KeyCode::Digit8,
            KeyCode::Digit9,
        ];
        let direct = digits
            .iter()
            .position(|&k| input.pressed(k))
            .filter(|&i| i < count && complete);
        let go = [
            KeyCode::KeyE,
            KeyCode::Space,
            KeyCode::Enter,
            KeyCode::NumpadEnter,
        ]
        .iter()
        .any(|&k| input.pressed(k));
        if go && !complete {
            line.time = f32::MAX / 4.0;
        } else if let Some(choice) = direct {
            story.runner.answer(Some(choice));
        } else if go {
            let reply = (count > 0).then_some(line.choice);
            story.runner.answer(reply);
        }
    }

    fn run_commands(&mut self, commands: Vec<Command>) {
        let mut reshown = false;
        let mut load = None;
        for command in commands {
            match command {
                Command::Door { door, action } => {
                    let open = match action {
                        DoorAction::Open => Some(true),
                        DoorAction::Close => Some(false),
                        DoorAction::Toggle => None,
                    };
                    match (self.scene.find(&door), &mut self.behaviors) {
                        (Some(index), Some(behaviors)) => behaviors.set_door(index, open),
                        _ => log::warn!("graph: no door \"{door}\" in this scene"),
                    }
                }
                Command::Show { object, visible } => match self.scene.find(&object) {
                    Some(index) => {
                        self.scene.objects[index].hidden = !visible;
                        reshown = true;
                    }
                    None => log::warn!("graph: no object \"{object}\" in this scene"),
                },
                Command::PlaySound { sound, at, volume } => {
                    let position = self.scene.find(&at).map_or_else(
                        || self.listener(),
                        |i| self.scene.objects[i].transform.position,
                    );
                    if let Some(audio) = &mut self.audio {
                        audio.play_sound(&sound, position, volume);
                    }
                }
                Command::LineSound { sound, speaker } => {
                    let position = self
                        .scene
                        .objects
                        .iter()
                        .find(|o| {
                            o.components
                                .iter()
                                .any(|c| matches!(c, Component::Character(ch) if ch.id == speaker))
                        })
                        .map_or_else(|| self.listener(), |o| o.transform.position);
                    if let Some(audio) = &mut self.audio {
                        audio.play_sound(&sound, position, 1.0);
                    }
                }
                Command::LoadScene(path) => load = Some(path),
                Command::Save => self.save_game(),
            }
        }
        if reshown {
            self.renderer.set_scene(&self.scene, &mut self.assets);
            self.rebuild_physics();
        }
        if let Some(path) = load {
            self.load_play_scene(&path);
        }
    }

    /// Loads another scene while the game runs; the player starts at its
    /// Player Start. Editor: Stop goes back to the scene being edited.
    pub(super) fn load_play_scene(&mut self, path: &str) {
        match Scene::load(&self.game_dir.join(path)) {
            Ok(scene) => {
                self.record_scene_state();
                self.scene = scene;
                self.play_scene = Some(PathBuf::from(path));
                if self.editor.is_none() {
                    self.scene_path = PathBuf::from(path);
                }
                self.behaviors = Some(Behaviors::start(&self.scene));
                self.player = Player::new(spawn_point(&self.scene), &self.settings);
                self.zone.reset();
                self.apply_scene_state();
                self.load_bake();
                self.load_lightmaps();
                log::info!("graph loaded {path}");
            }
            Err(err) => {
                log::error!("{err}");
                if let Some(editor) = &mut self.editor {
                    editor.show_error(err.to_string());
                }
            }
        }
    }

    pub(super) fn save_game(&mut self) {
        self.record_scene_state();
        let scene = self.scene_key();
        let Some(story) = &mut self.story else { return };
        let saved = SaveGame {
            scene,
            position: self.player.feet,
            yaw: self.player.yaw,
            graphs: story.runner.state(),
            world: story.world.clone(),
        };
        match save::write(&self.game_dir, &saved) {
            Ok(path) => {
                log::info!("saved the game to {}", path.display());
                story.toast("GAME SAVED");
            }
            Err(err) => {
                log::error!("saving failed: {err}");
                story.toast("COULDN'T SAVE");
            }
        }
    }

    /// Carries on from a saved game.
    pub(super) fn load_game(&mut self, saved: SaveGame) {
        if !saved.scene.is_empty() && saved.scene != self.scene_key() {
            self.load_play_scene(&saved.scene);
        }
        if let Some(story) = &mut self.story {
            story.world = saved.world;
            story.runner.restore(saved.graphs);
            story.toast("GAME LOADED");
        }
        self.apply_scene_state();
        self.player = Player::new(
            Spawn {
                position: saved.position,
                yaw: saved.yaw,
            },
            &self.settings,
        );
        self.mode = CameraMode::Player;
    }

    /// The air jumps the controller may use now: none until the ability's
    /// flag (if it needs one) is true.
    pub(super) fn update_abilities(&mut self) {
        let jump = &self.settings.jump;
        let unlocked = jump.air_jump_flag.is_empty()
            || self
                .story
                .as_ref()
                .is_some_and(|s| s.runner.flags.is_true(&jump.air_jump_flag));
        self.player.air_jumps = if unlocked { jump.air_jumps } else { 0 };
    }

    /// The speaker's name as shown now: renamed by a graph, else the
    /// character's own, else the id.
    pub(super) fn speaker_name(&self, id: &str) -> String {
        if let Some(name) = self.story.as_ref().and_then(|s| s.runner.names.get(id)) {
            return name.clone();
        }
        self.scene
            .objects
            .iter()
            .find_map(|o| {
                o.components.iter().find_map(|c| match c {
                    Component::Character(ch) if ch.id == id => Some(ch.name.clone()),
                    _ => None,
                })
            })
            .unwrap_or_else(|| id.to_owned())
    }
}
