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
use crate::graph::run::{Command, Event, GraphRunner, LineView, World};
use crate::graph::{DoorAction, key};
use crate::player::{Player, Spawn};
use crate::project::Project;
use crate::save::{self, SaveGame, SceneState, WorldState};
use crate::scene::{Component, ObjectRef, Scene};
use glam::Vec3;
use std::path::PathBuf;
use winit::keyboard::KeyCode;

pub(super) struct Story {
    pub runner: GraphRunner,
    /// The abilities' scripts that are on.
    pub scripts: crate::script::Scripts,
    pub world: WorldState,
    pub style: DialogueStyle,
    /// Whether the style's font loaded (`None`: not tried yet).
    pub custom_font: Option<bool>,
    /// The line on screen.
    pub line: Option<LineUi>,
    pub map_open: bool,
    /// Play mode: the test drawer is open (F3); the editor keeps its own.
    pub debug_open: bool,
    /// The test drawer's tab.
    pub tab: super::test_drawer::Tab,
    /// What happened, with the seconds since the game started.
    pub log: Vec<(f32, String)>,
    clock: f32,
    /// The zone the player was in last frame (for ENTER ZONE events).
    last_zone: Option<String>,
    pub debug_view: CanvasView,
    /// Which running graph the debug view shows.
    pub debug_graph: usize,
    /// A short message ("GAME SAVED") and the seconds it stays.
    pub toast: Option<(String, f32)>,
    /// Objects the player used and picked up this frame.
    /// What happened since the graphs last ran (uses, pickups, signals).
    pub events: Vec<Event>,
    /// The pressure plate the player stood on last frame.
    plate: Option<usize>,
    /// `VisibleWhen` objects: whether their condition held last frame, so
    /// only a change shows or hides them.
    conditions: std::collections::HashMap<usize, bool>,
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
        let defs = FlagDefs::load(&self.game_dir);
        let mut runner = GraphRunner::new(self.game_dir.clone(), defs.start());
        runner.bind_abilities(defs.abilities());
        self.story = Some(Story {
            runner,
            scripts: crate::script::Scripts::new(self.game_dir.clone()),
            world: WorldState::default(),
            style: settings
                .as_ref()
                .map(|s| s.dialogue.clone())
                .unwrap_or_default(),
            custom_font: None,
            line: None,
            map_open: false,
            debug_open: false,
            tab: Default::default(),
            log: Vec::new(),
            clock: 0.0,
            last_zone: None,
            debug_view: CanvasView::default(),
            debug_graph: 0,
            toast: None,
            events: Vec::new(),
            plate: None,
            conditions: Default::default(),
        });
        self.apply_scene_state();
        if resume && let Some(saved) = save::load(&self.game_dir) {
            self.load_game(saved);
            return;
        }
        self.enter_scene_graph();
    }

    /// Starts this scene's graph, or carries on with it if the player was
    /// here before.
    fn enter_scene_graph(&mut self) {
        let scene = self.scene_key();
        let graph = crate::graph::scene_graph(&scene);
        if !self.game_dir.join(&graph).is_file() {
            return;
        }
        let Some(story) = &mut self.story else { return };
        match story.world.scene_graphs.remove(&scene) {
            Some(paused) => story.runner.resume(paused),
            None => {
                story.runner.start(&graph);
            }
        }
    }

    /// Pauses this scene's graph while the player is elsewhere.
    fn leave_scene_graph(&mut self) {
        let scene = self.scene_key();
        let graph = crate::graph::scene_graph(&scene);
        let Some(story) = &mut self.story else { return };
        if story.runner.is_running(&graph) || !story.world.scene_graphs.contains_key(&scene) {
            let paused = story.runner.suspend(&graph);
            story.world.scene_graphs.insert(scene, paused);
        }
        // The scene's logic leaves memory with it; it is read again on return.
        story.runner.unload_unused();
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
            if object.hidden || visible_when(object).is_some() {
                state.objects.entry(object.id).or_default().hidden = Some(object.hidden);
            }
        }
        if let Some(behaviors) = &self.behaviors {
            for (index, open) in behaviors.doors() {
                let id = self.scene.objects[index].id;
                state.objects.entry(id).or_default().door_open = Some(open);
            }
        }
        story.world.scenes.insert(key, state);
    }

    /// The scene as the player left it: hidden objects, open doors, gates.
    fn apply_scene_state(&mut self) {
        let key = self.scene_key();
        let Some(story) = &mut self.story else { return };
        let state = story.world.scenes.get(&key);
        story.conditions.clear();
        for (index, object) in self.scene.objects.iter_mut().enumerate() {
            // A flag condition decides; what a graph showed or hid (saved)
            // counts for everything else.
            let condition = visible_when(object).cloned();
            let shown = condition.as_ref().map(|c| c.shown(&story.runner.flags));
            object.hidden = shown == Some(false);
            if let Some(shown) = shown {
                story.conditions.insert(index, shown);
            }
            let flagged = condition.is_some_and(|c| !c.condition.flag.0.is_empty());
            let saved = state.filter(|_| !flagged).and_then(|s| {
                let by_id = s.objects.get(&object.id).and_then(|o| o.hidden);
                // An old save, by name.
                by_id.or_else(|| s.hidden.get(&object.name).copied())
            });
            if let Some(hidden) = saved {
                object.hidden = hidden;
            }
        }
        if let (Some(state), Some(behaviors)) = (state, &mut self.behaviors) {
            let by_id = state.objects.iter().filter_map(|(id, o)| {
                let index = self.scene.objects.iter().position(|x| x.id == *id)?;
                Some((index, o.door_open?))
            });
            let by_name = state
                .doors
                .iter()
                .filter_map(|(name, &open)| Some((self.scene.find(name)?, open)));
            for (index, open) in by_name.chain(by_id) {
                behaviors.snap_door(index, open);
            }
        }
        self.renderer.set_scene(&self.scene, &mut self.assets);
        self.rebuild_physics();
    }

    /// `VisibleWhen` objects whose condition's answer changed are shown or
    /// hidden. Returns them.
    fn update_visibility(&mut self) -> Vec<usize> {
        let Some(story) = &mut self.story else {
            return Vec::new();
        };
        let mut changed = Vec::new();
        for (index, object) in self.scene.objects.iter_mut().enumerate() {
            let Some(condition) = visible_when(object) else {
                continue;
            };
            let shown = condition.shown(&story.runner.flags);
            if story.conditions.insert(index, shown) != Some(shown) {
                object.hidden = !shown;
                changed.push(index);
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
        // Every use is an event graphs can wait for.
        if let Some(story) = &mut self.story {
            story
                .events
                .push(Event::Used(ObjectRef::to(&self.scene.objects[index])));
        }
        match action {
            Use::Switch => return Some(index),
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
        let picked = ObjectRef::to(object);
        if let Some(story) = &mut self.story {
            story.events.push(Event::PickedUp(picked));
            if let Some(pickup) = &pickup {
                story.runner.flags.set(&pickup.flag.0, "true");
            }
        }
        if let (Some(pickup), Some(audio)) = (&pickup, &mut self.audio)
            && !pickup.sound.0.is_empty()
        {
            audio.play_sound(&pickup.sound.0, position, 1.0);
        }
        self.reshow(index);
    }

    /// An object was shown or hidden: updates just that object in the
    /// renderer and physics (no full rebuild).
    fn reshow(&mut self, index: usize) {
        self.renderer
            .update_object(&self.scene, index, &mut self.assets);
        self.physics
            .set_hidden(index, self.scene.objects[index].hidden);
    }

    /// Moves the story on by a frame: dialogue keys, the graphs, what they
    /// ask for, gates and visited zones.
    pub(super) fn update_story(&mut self, dt: f32) {
        let plate = self.behaviors.as_ref().and_then(Behaviors::plate);
        let Some(story) = &mut self.story else {
            return;
        };
        // Stepping onto a pressure plate uses it.
        if plate != story.plate {
            story.plate = plate;
            if let Some(index) = plate {
                let used = ObjectRef::to(&self.scene.objects[index]);
                story.events.push(Event::Used(used));
            }
        }
        self.dialogue_keys();
        let scene_key = self.scene_key();
        let body = self.body();
        let zone = self
            .zone_map
            .zone_at(body)
            .map(|z| self.scene.zones[z].name.clone());
        let triggers: Vec<ObjectRef> = self
            .scene
            .objects
            .iter()
            .filter(|o| {
                o.components.iter().any(|c| match c {
                    Component::TriggerZone(t) => t.contains(&o.transform, body),
                    _ => false,
                })
            })
            .map(ObjectRef::to)
            .collect();
        let Some(story) = &mut self.story else { return };
        if let Some(zone) = &zone {
            story.world.visit(&scene_key, zone);
        }
        let events = std::mem::take(&mut story.events);
        story.clock += dt;
        let clock = story.clock;
        if zone != story.last_zone {
            if let Some(zone) = &zone {
                story.log.push((clock, format!("ENTER ZONE {zone}")));
            }
            story.last_zone = zone.clone();
        }
        for event in &events {
            let entry = match event {
                Event::Used(object) => format!("USED {}", object.name),
                Event::PickedUp(object) => format!("PICKED UP {}", object.name),
                Event::Signal(name) => format!("SIGNAL {name}"),
            };
            story.log.push((clock, entry));
        }
        let world = World {
            scene: &scene_key,
            zone: zone.as_deref(),
            triggers: &triggers,
            events: &events,
        };
        let commands = story.runner.update(&world, dt);
        for entry in std::mem::take(&mut story.runner.log) {
            story.log.push((clock, entry));
        }
        let overflow = story.log.len().saturating_sub(200);
        story.log.drain(..overflow);
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
        for index in self.update_visibility() {
            self.reshow(index);
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
        let mut reshown = Vec::new();
        let mut load = None;
        for command in commands {
            match command {
                Command::Door { door, action } => {
                    let open = match action {
                        DoorAction::Open => Some(true),
                        DoorAction::Close => Some(false),
                        DoorAction::Toggle => None,
                    };
                    match (self.scene.resolve(&door), &mut self.behaviors) {
                        (Some(index), Some(behaviors)) => behaviors.set_door(index, open),
                        _ => log::warn!("graph: no door \"{}\" in this scene", door.name),
                    }
                }
                Command::Show { object, visible } => match self.scene.resolve(&object) {
                    Some(index) => {
                        if self.scene.objects[index].hidden == visible {
                            self.scene.objects[index].hidden = !visible;
                            reshown.push(index);
                        }
                    }
                    None => log::warn!("graph: no object \"{}\" in this scene", object.name),
                },
                Command::PlaySound { sound, at, volume } => {
                    let position = self.scene.resolve(&at).map_or_else(
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
        for index in reshown {
            self.reshow(index);
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
                self.leave_scene_graph();
                self.scene = scene;
                self.play_scene = Some(PathBuf::from(path));
                if self.editor.is_none() {
                    self.scene_path = PathBuf::from(path);
                }
                self.behaviors = Some(Behaviors::start(&self.scene));
                if let Some(story) = &mut self.story {
                    story.scripts.reset_objects();
                }
                self.player = Player::new(spawn_point(&self.scene), &self.settings);
                self.zone.reset();
                self.apply_scene_state();
                self.load_bake();
                self.load_lightmaps();
                self.enter_scene_graph();
                log::info!("scene loaded {path}");
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

    /// Runs the scripts for this frame, after the controller moved: the
    /// abilities the graphs turned on, then the scripts on objects in the
    /// simulation radius. What they change goes back to the player and the
    /// objects.
    pub(super) fn run_scripts(&mut self, input: &crate::input::Input, dt: f32) {
        let Some(story) = &mut self.story else { return };
        story.scripts.sync(&story.runner.active_abilities());
        let mut view = self.player.script_view(input, &self.settings);
        let mut objects: Vec<crate::script::ObjectView> = self
            .scene
            .objects
            .iter()
            .enumerate()
            .filter(|(i, _)| self.active.object(*i))
            .filter_map(|(index, object)| {
                let script = object.components.iter().find_map(|c| match c {
                    Component::Script(s) if !s.script.0.is_empty() => Some(s.script.0.clone()),
                    _ => None,
                })?;
                // A floating object's script works with where it floats
                // around; the bob is added back after.
                let lift = self
                    .behaviors
                    .as_ref()
                    .map_or(0.0, |b| b.lift(&self.scene, index));
                Some(crate::script::ObjectView {
                    index,
                    script,
                    name: object.name.clone(),
                    position: object.transform.position - glam::Vec3::Y * lift,
                    rotation: object.transform.rotation,
                    scale: object.transform.scale,
                    visible: !object.hidden,
                })
            })
            .collect();
        story
            .scripts
            .update(&mut view, &mut objects, &mut story.runner.flags, dt);
        let signals = std::mem::take(&mut story.scripts.signals);
        story.events.extend(signals.into_iter().map(Event::Signal));
        if self.mode == CameraMode::Player {
            self.player.apply_script(&view, &self.settings);
        }
        let mut reshown = Vec::new();
        for changed in &objects {
            let object = &mut self.scene.objects[changed.index];
            if object.hidden == changed.visible {
                object.hidden = !changed.visible;
                reshown.push(changed.index);
            }
            let base = crate::scene::Transform {
                position: changed.position,
                rotation: changed.rotation,
                scale: changed.scale,
            };
            let mut transform = base;
            if let Some(behaviors) = &mut self.behaviors
                && object_floats(&self.scene.objects[changed.index])
            {
                behaviors.set_rest(changed.index, base);
                transform.position.y += behaviors.lift(&self.scene, changed.index);
            }
            let object = &mut self.scene.objects[changed.index];
            if transform != object.transform {
                object.transform = transform;
                self.renderer
                    .update_object(&self.scene, changed.index, &mut self.assets);
                self.physics
                    .move_object(changed.index, &self.scene.objects[changed.index].transform);
            }
        }
        for index in reshown {
            self.reshow(index);
        }
        let Some(story) = &mut self.story else { return };
        let errors = std::mem::take(&mut story.scripts.errors);
        for error in errors {
            story
                .log
                .push((story.clock, format!("SCRIPT ERROR {error}")));
            story.toast("SCRIPT ERROR: SEE THE LOG");
            if let Some(editor) = &mut self.editor {
                editor.show_error(error);
            }
        }
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

/// The object's `VisibleWhen`, if it has one.
fn visible_when(object: &crate::scene::Object) -> Option<&crate::scene::VisibleWhen> {
    object.components.iter().find_map(|c| match c {
        Component::VisibleWhen(v) => Some(v),
        _ => None,
    })
}

fn object_floats(object: &crate::scene::Object) -> bool {
    object
        .components
        .iter()
        .any(|c| matches!(c, Component::Floating(_)))
}
