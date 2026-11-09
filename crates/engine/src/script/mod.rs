//! Scripts: small Rhai programs in the project's `scripts/` folder. A
//! script runs in one of two ways:
//!
//! - **As an ability** of the player: a graph's ABILITY node turns it on
//!   or off. The engine calls `start()` once when it's turned on and
//!   `update(player, dt)` every frame while it's on.
//! - **On an object**, through its Script behavior: `start(object)` once
//!   and `update(object, dt)` every frame while the game plays and the
//!   object is in the simulation radius.
//!
//! ```rhai
//! fn start() { this.used = 0; }   // `this`: the script's own memory
//!
//! fn update(player, dt) {
//!     if player.grounded { this.used = 0; }
//!     if player.pressed("space") && !player.grounded && !player.jumped && this.used < 1 {
//!         player.jump();
//!         this.used += 1;
//!     }
//! }
//! ```
//!
//! Each ability and each object has its own memory, so one script can run
//! on many objects. `player()` gives any script the player; `flag` and
//! `set_flag` read and set the game's flags, so scripts and graphs talk to
//! each other. Scripts reload when their file changes. A script with an
//! error stops (the error is logged and shown) until the file is fixed.

pub mod format;

use crate::graph::flags::{FlagValue, Flags};
use crate::input::Input;
use glam::Vec3;
use rhai::{AST, CallFnOptions, Dynamic, Engine, EvalAltResult, Map, Scope};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::SystemTime;
use winit::keyboard::KeyCode;

/// Steps one call may take before it's stopped (an endless loop would
/// freeze the game).
const MAX_OPERATIONS: u64 = 200_000;

/// What a script sees of the player, and what it may change.
#[derive(Debug, Clone, Default)]
pub struct PlayerView {
    pub grounded: bool,
    /// The controller jumped this frame (off the ground or in coyote time).
    pub jumped: bool,
    pub crouching: bool,
    pub sprinting: bool,
    /// Feet position (read only).
    pub position: Vec3,
    pub velocity: Vec3,
    /// The upward speed of a normal jump (from `player.ron`).
    pub jump_speed: f32,
    /// A script asked for a jump this frame.
    pub jump: bool,
    pub input: Input,
}

/// What a script sees of the object it runs on, and may change.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ObjectView {
    /// Its index in the scene (the runtime's; scripts don't see it).
    pub index: usize,
    /// The script running on it.
    pub script: String,
    pub name: String,
    pub position: Vec3,
    /// Degrees around X, Y and Z.
    pub rotation: Vec3,
    pub scale: Vec3,
    pub visible: bool,
}

/// The `player` value scripts get: shared, so their changes come back.
#[derive(Clone)]
struct PlayerHandle(Rc<RefCell<PlayerView>>);

/// The `object` value object scripts get.
#[derive(Clone)]
struct ObjectHandle(Rc<RefCell<ObjectView>>);

/// What every script can reach this frame through plain functions
/// (`player()`, `flag`, `set_flag`).
#[derive(Default)]
struct Shared {
    flags: Flags,
    set: Vec<(String, String)>,
    player: Option<PlayerHandle>,
}

struct Compiled {
    ast: Option<AST>,
    modified: Option<SystemTime>,
    /// Goes up on every reload, so running copies start over.
    generation: u64,
}

/// One running copy of a script: an ability, or a script on one object.
struct Instance {
    /// The script's `this`: a map it fills as it likes.
    memory: Dynamic,
    started: bool,
    generation: u64,
}

impl Instance {
    fn new(generation: u64) -> Self {
        Self {
            memory: Dynamic::from_map(Map::new()),
            started: false,
            generation,
        }
    }
}

pub struct Scripts {
    engine: Engine,
    /// The project folder scripts are read from.
    dir: PathBuf,
    compiled: BTreeMap<String, Compiled>,
    /// Abilities that are on, by script path.
    abilities: BTreeMap<String, Option<Instance>>,
    /// Scripts on objects, by object index and script path.
    objects: BTreeMap<(usize, String), Instance>,
    shared: Rc<RefCell<Shared>>,
    /// Errors since last taken, for the editor and the test drawer.
    pub errors: Vec<String>,
}

impl Scripts {
    pub fn new(dir: PathBuf) -> Self {
        let shared = Rc::new(RefCell::new(Shared::default()));
        let mut engine = Engine::new();
        engine.set_max_operations(MAX_OPERATIONS);
        engine.on_print(|text| log::info!("script: {text}"));
        register_vec(&mut engine);
        register_player(&mut engine);
        register_object(&mut engine);
        let read = shared.clone();
        engine.register_fn("flag", move |name: &str| -> Dynamic {
            match read.borrow().flags.get(name) {
                FlagValue::Bool(b) => b.into(),
                FlagValue::Number(n) => (n as f64).into(),
                FlagValue::Text(t) => t.into(),
            }
        });
        let write = shared.clone();
        engine.register_fn("set_flag", move |name: &str, value: Dynamic| {
            let text = value.to_string();
            let mut shared = write.borrow_mut();
            shared.flags.set(name, &text);
            shared.set.push((name.to_owned(), text));
        });
        let player = shared.clone();
        engine.register_fn("player", move || -> Dynamic {
            player
                .borrow()
                .player
                .clone()
                .map_or(Dynamic::UNIT, Dynamic::from)
        });
        Self {
            engine,
            dir,
            compiled: BTreeMap::new(),
            abilities: BTreeMap::new(),
            objects: BTreeMap::new(),
            shared,
            errors: Vec::new(),
        }
    }

    /// The abilities that are on from now on: new ones start fresh, the
    /// others stop.
    pub fn sync(&mut self, enabled: &BTreeSet<String>) {
        self.abilities.retain(|path, _| enabled.contains(path));
        for path in enabled {
            self.abilities.entry(path.clone()).or_insert(None);
        }
    }

    /// Forgets the scripts on objects (another scene was loaded): they
    /// start over.
    pub fn reset_objects(&mut self) {
        self.objects.clear();
    }

    /// Runs this frame: the abilities on the player, then the scripts on
    /// `objects`. Their changes are in `player` and `objects` afterwards;
    /// flags they set are in `flags`.
    pub fn update(
        &mut self,
        player: &mut PlayerView,
        objects: &mut [ObjectView],
        flags: &mut Flags,
        dt: f32,
    ) {
        if self.abilities.is_empty() && objects.is_empty() {
            return;
        }
        let handle = PlayerHandle(Rc::new(RefCell::new(std::mem::take(player))));
        *self.shared.borrow_mut() = Shared {
            flags: flags.clone(),
            set: Vec::new(),
            player: Some(handle.clone()),
        };
        let dt = dt as f64;

        let paths: Vec<String> = self.abilities.keys().cloned().collect();
        for path in paths {
            let Some((ast, generation)) = self.load(&path) else {
                continue;
            };
            let Some(slot) = self.abilities.get_mut(&path) else {
                continue;
            };
            let instance = slot.get_or_insert_with(|| Instance::new(generation));
            let result = run(
                &self.engine,
                &ast,
                instance,
                generation,
                (),
                (handle.clone(), dt),
            );
            self.check(&path, result);
        }

        for view in objects.iter_mut() {
            let path = view.script.clone();
            let Some((ast, generation)) = self.load(&path) else {
                continue;
            };
            let index = view.index;
            let object = ObjectHandle(Rc::new(RefCell::new(std::mem::take(view))));
            let instance = self
                .objects
                .entry((index, path.clone()))
                .or_insert_with(|| Instance::new(generation));
            let result = run(
                &self.engine,
                &ast,
                instance,
                generation,
                (object.clone(),),
                (object.clone(), dt),
            );
            *view = object.0.borrow().clone();
            self.check(&path, result);
        }

        *player = handle.0.borrow().clone();
        let mut shared = self.shared.borrow_mut();
        shared.player = None;
        for (name, text) in shared.set.drain(..) {
            flags.set(&name, &text);
        }
    }

    /// A script's compiled form and generation, read again when it's new
    /// or its file changed (its running copies then start over); `None`
    /// while it has an error.
    fn load(&mut self, path: &str) -> Option<(AST, u64)> {
        let file = self.dir.join(path);
        let modified = std::fs::metadata(&file).and_then(|m| m.modified()).ok();
        let compiled = self.compiled.entry(path.to_owned()).or_insert(Compiled {
            ast: None,
            // Never a file's time: compiled on first use.
            modified: Some(SystemTime::UNIX_EPOCH),
            generation: 0,
        });
        if compiled.modified != modified {
            compiled.modified = modified;
            compiled.generation += 1;
            let result = std::fs::read_to_string(&file)
                .map_err(|err| err.to_string())
                .and_then(|text| self.engine.compile(text).map_err(|err| err.to_string()));
            match result {
                Ok(ast) => {
                    log::info!("script loaded {path}");
                    compiled.ast = Some(ast);
                }
                Err(err) => {
                    compiled.ast = None;
                    self.report(format!("{path}: {err}"));
                }
            }
        }
        let compiled = self.compiled.get(path)?;
        Some((compiled.ast.clone()?, compiled.generation))
    }

    /// A script that failed stops (all its copies) until its file changes.
    fn check(&mut self, path: &str, result: Result<(), Box<EvalAltResult>>) {
        if let Err(err) = result {
            if let Some(compiled) = self.compiled.get_mut(path) {
                compiled.ast = None;
            }
            self.report(format!("{path}: {err}"));
        }
    }

    fn report(&mut self, error: String) {
        log::error!("{error}");
        self.errors.push(error);
    }
}

/// The first syntax error in a script's text, with its line (from 1).
pub fn syntax_error(text: &str) -> Option<(usize, String)> {
    let err = Engine::new().compile(text).err()?;
    let line = err.1.line().unwrap_or(1);
    Some((line, err.0.to_string()))
}

/// Runs one copy of a script for a frame: `start` first if it hasn't run
/// (or the script was reloaded since), then `update`.
fn run(
    engine: &Engine,
    ast: &AST,
    instance: &mut Instance,
    generation: u64,
    start_args: impl rhai::FuncArgs,
    update_args: impl rhai::FuncArgs,
) -> Result<(), Box<EvalAltResult>> {
    if instance.generation != generation {
        *instance = Instance::new(generation);
    }
    if !instance.started {
        instance.started = true;
        call(engine, ast, &mut instance.memory, "start", start_args)?;
    }
    call(engine, ast, &mut instance.memory, "update", update_args)
}

/// Calls the script's function `name` with `this` bound to its memory;
/// a script without that function (taking that many values) is fine.
fn call(
    engine: &Engine,
    ast: &AST,
    memory: &mut Dynamic,
    name: &str,
    args: impl rhai::FuncArgs,
) -> Result<(), Box<EvalAltResult>> {
    let mut values: Vec<Dynamic> = Vec::new();
    args.parse(&mut values);
    if !ast
        .iter_functions()
        .any(|f| f.name == name && f.params.len() == values.len())
    {
        return Ok(());
    }
    let options = CallFnOptions::new().eval_ast(false).bind_this_ptr(memory);
    engine
        .call_fn_with_options::<Dynamic>(options, &mut Scope::new(), ast, name, values)
        .map(|_| ())
}

/// The object a script runs on: its name, where it is, how it's turned
/// and scaled, and whether it's shown.
fn register_object(engine: &mut Engine) {
    engine
        .register_type_with_name::<ObjectHandle>("Object")
        .register_get("name", |o: &mut ObjectHandle| o.0.borrow().name.clone())
        .register_get("visible", |o: &mut ObjectHandle| o.0.borrow().visible)
        .register_set("visible", |o: &mut ObjectHandle, v: bool| {
            o.0.borrow_mut().visible = v
        });
    macro_rules! vector {
        ($name:literal, $field:ident) => {
            engine
                .register_get($name, |o: &mut ObjectHandle| o.0.borrow().$field)
                .register_set($name, |o: &mut ObjectHandle, v: Vec3| {
                    o.0.borrow_mut().$field = v
                });
        };
    }
    vector!("position", position);
    vector!("rotation", rotation);
    vector!("scale", scale);
}

/// A whole or decimal number as `f32` (Rhai keeps the two apart).
fn number(value: &Dynamic) -> Result<f32, Box<EvalAltResult>> {
    if let Ok(f) = value.as_float() {
        Ok(f as f32)
    } else if let Ok(i) = value.as_int() {
        Ok(i as f32)
    } else {
        Err(format!("expected a number, got {}", value.type_name()).into())
    }
}

/// `vec3(x, y, z)` with `.x`, `.y`, `.z`. Setters take whole numbers too,
/// so `v.y = 5` works like `v.y = 5.0`.
fn register_vec(engine: &mut Engine) {
    engine
        .register_type_with_name::<Vec3>("Vec3")
        .register_fn(
            "vec3",
            |x: Dynamic, y: Dynamic, z: Dynamic| -> Result<Vec3, Box<EvalAltResult>> {
                Ok(Vec3::new(number(&x)?, number(&y)?, number(&z)?))
            },
        )
        .register_fn("to_string", |v: &mut Vec3| v.to_string())
        .register_fn("to_debug", |v: &mut Vec3| v.to_string())
        .register_fn("+", |a: Vec3, b: Vec3| a + b)
        .register_fn("-", |a: Vec3, b: Vec3| a - b)
        .register_fn("*", |a: Vec3, s: f64| a * s as f32)
        .register_fn("*", |a: Vec3, s: i64| a * s as f32)
        .register_fn("/", |a: Vec3, s: f64| a / s as f32)
        .register_fn("/", |a: Vec3, s: i64| a / s as f32)
        .register_fn("length", |v: &mut Vec3| v.length() as f64);
    macro_rules! axis {
        ($name:literal, $field:ident) => {
            engine
                .register_get($name, |v: &mut Vec3| v.$field as f64)
                .register_set($name, |v: &mut Vec3, x: f64| v.$field = x as f32)
                .register_set($name, |v: &mut Vec3, x: i64| v.$field = x as f32);
        };
    }
    axis!("x", x);
    axis!("y", y);
    axis!("z", z);
}

fn register_player(engine: &mut Engine) {
    engine.register_type_with_name::<PlayerHandle>("Player");
    macro_rules! get {
        ($name:literal, $field:ident) => {
            engine.register_get($name, |p: &mut PlayerHandle| p.0.borrow().$field);
        };
    }
    get!("grounded", grounded);
    get!("jumped", jumped);
    get!("crouching", crouching);
    get!("sprinting", sprinting);
    get!("position", position);
    get!("velocity", velocity);
    engine
        .register_get("jump_speed", |p: &mut PlayerHandle| {
            p.0.borrow().jump_speed as f64
        })
        .register_set("velocity", |p: &mut PlayerHandle, v: Vec3| {
            p.0.borrow_mut().velocity = v
        })
        .register_fn("jump", |p: &mut PlayerHandle| p.0.borrow_mut().jump = true)
        .register_fn(
            "pressed",
            |p: &mut PlayerHandle, key: &str| -> Result<bool, Box<EvalAltResult>> {
                Ok(p.0.borrow().input.pressed(key_code(key)?))
            },
        )
        .register_fn(
            "held",
            |p: &mut PlayerHandle, key: &str| -> Result<bool, Box<EvalAltResult>> {
                Ok(p.0.borrow().input.held(key_code(key)?))
            },
        );
}

/// A key by the name scripts use: a letter or digit, `space`, `shift`,
/// `ctrl`, `alt`, `tab`, `enter`, `up` / `down` / `left` / `right`.
fn key_code(name: &str) -> Result<KeyCode, Box<EvalAltResult>> {
    use KeyCode::*;
    const LETTERS: [KeyCode; 26] = [
        KeyA, KeyB, KeyC, KeyD, KeyE, KeyF, KeyG, KeyH, KeyI, KeyJ, KeyK, KeyL, KeyM, KeyN, KeyO,
        KeyP, KeyQ, KeyR, KeyS, KeyT, KeyU, KeyV, KeyW, KeyX, KeyY, KeyZ,
    ];
    const DIGITS: [KeyCode; 10] = [
        Digit0, Digit1, Digit2, Digit3, Digit4, Digit5, Digit6, Digit7, Digit8, Digit9,
    ];
    let lower = name.to_ascii_lowercase();
    let key = match lower.as_str() {
        "space" => Some(Space),
        "shift" => Some(ShiftLeft),
        "ctrl" => Some(ControlLeft),
        "alt" => Some(AltLeft),
        "tab" => Some(Tab),
        "enter" => Some(Enter),
        "up" => Some(ArrowUp),
        "down" => Some(ArrowDown),
        "left" => Some(ArrowLeft),
        "right" => Some(ArrowRight),
        one if one.len() == 1 => {
            let c = one.as_bytes()[0];
            match c {
                b'a'..=b'z' => Some(LETTERS[(c - b'a') as usize]),
                b'0'..=b'9' => Some(DIGITS[(c - b'0') as usize]),
                _ => None,
            }
        }
        _ => None,
    };
    key.ok_or_else(|| format!("no key named \"{name}\"").into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scripts_with(name: &str, text: &str) -> (Scripts, PathBuf) {
        let dir = std::env::temp_dir().join(format!("engine-script-{name}-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("scripts")).unwrap();
        std::fs::write(dir.join("scripts/a.rhai"), text).unwrap();
        let mut scripts = Scripts::new(dir.clone());
        scripts.sync(&BTreeSet::from(["scripts/a.rhai".to_string()]));
        (scripts, dir)
    }

    fn frame(scripts: &mut Scripts, view: &mut PlayerView, flags: &mut Flags) {
        scripts.update(view, &mut [], flags, 1.0 / 60.0);
        view.jump = false;
        view.input.end_frame();
    }

    #[test]
    fn the_sandbox_double_jump_jumps_once_in_the_air() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sandbox");
        let mut scripts = Scripts::new(dir);
        scripts.sync(&BTreeSet::from(["scripts/double_jump.rhai".to_string()]));
        let mut flags = Flags::default();
        let mut view = PlayerView {
            grounded: true,
            ..Default::default()
        };
        frame(&mut scripts, &mut view, &mut flags);
        assert!(scripts.errors.is_empty(), "{:?}", scripts.errors);

        // The controller's own jump off the ground: the script stays out of it.
        let mut press = |scripts: &mut Scripts, view: &mut PlayerView, jumped: bool| {
            view.grounded = false;
            view.jumped = jumped;
            view.input.key_up(KeyCode::Space);
            view.input.key_down(KeyCode::Space);
            scripts.update(view, &mut [], &mut flags, 0.016);
            let jumped = view.jump;
            view.jump = false;
            view.input.end_frame();
            jumped
        };
        assert!(
            !press(&mut scripts, &mut view, true),
            "not on the ground jump"
        );
        assert!(press(&mut scripts, &mut view, false), "one jump in the air");
        assert!(!press(&mut scripts, &mut view, false), "only one");
        assert!(scripts.errors.is_empty(), "{:?}", scripts.errors);
    }

    #[test]
    fn scripts_change_velocity_and_flags() {
        let (mut scripts, dir) = scripts_with(
            "flags",
            r#"
            fn update(player, dt) {
                player.velocity.y = 3;
                if flag("open") { set_flag("count", flag("count") + 1); }
            }
            "#,
        );
        let mut flags = Flags::default();
        flags.set("open", "true");
        flags.set("count", "1");
        let mut view = PlayerView::default();
        frame(&mut scripts, &mut view, &mut flags);
        assert!(scripts.errors.is_empty(), "{:?}", scripts.errors);
        assert_eq!(view.velocity.y, 3.0);
        assert_eq!(flags.get("count").as_number(), 2.0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn scripts_on_objects_move_them_each_with_its_own_memory() {
        let (mut scripts, dir) = scripts_with(
            "objects",
            r#"
            fn start(object) { this.steps = 0; }
            fn update(object, dt) {
                this.steps += 1;
                object.position.y = this.steps;
                object.rotation = vec3(0, 90, 0);
                if (player().position - object.position).length() > 5.0 { object.visible = false; }
            }
            "#,
        );
        scripts.sync(&BTreeSet::new());
        let lamp = |index: usize, x: f32| ObjectView {
            index,
            script: "scripts/a.rhai".into(),
            name: format!("lamp {index}"),
            position: Vec3::new(x, 0.0, 0.0),
            visible: true,
            ..Default::default()
        };
        let mut objects = vec![lamp(0, 0.0), lamp(1, 10.0)];
        let mut player = PlayerView::default();
        let mut flags = Flags::default();
        scripts.update(&mut player, &mut objects, &mut flags, 0.1);
        assert!(scripts.errors.is_empty(), "{:?}", scripts.errors);
        assert!(
            objects[0].visible && !objects[1].visible,
            "the far one hides"
        );
        objects.truncate(1);
        scripts.update(&mut player, &mut objects, &mut flags, 0.1);
        assert!(scripts.errors.is_empty(), "{:?}", scripts.errors);
        assert_eq!(objects[0].position.y, 2.0, "its own count");
        assert_eq!(objects[0].rotation.y, 90.0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_sandbox_watcher_turns_to_the_player() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sandbox");
        let mut scripts = Scripts::new(dir);
        let mut objects = vec![ObjectView {
            index: 0,
            script: "scripts/watcher.rhai".into(),
            position: Vec3::new(0.0, 1.0, 0.0),
            scale: Vec3::ONE,
            visible: true,
            ..Default::default()
        }];
        // The player straight to the east (+X).
        let mut player = PlayerView {
            position: Vec3::new(5.0, 0.0, 0.0),
            ..Default::default()
        };
        scripts.update(&mut player, &mut objects, &mut Flags::default(), 0.5);
        assert!(scripts.errors.is_empty(), "{:?}", scripts.errors);
        assert!(
            (objects[0].rotation.y.abs() - 90.0).abs() < 0.01,
            "{}",
            objects[0].rotation
        );
        assert!((objects[0].position.y - 1.0).abs() <= 0.1);
    }

    #[test]
    fn a_broken_script_reports_and_stops() {
        let (mut scripts, dir) = scripts_with("broken", "fn update(player, dt) { player.fly(); }");
        let mut view = PlayerView::default();
        let mut flags = Flags::default();
        frame(&mut scripts, &mut view, &mut flags);
        assert_eq!(scripts.errors.len(), 1);
        frame(&mut scripts, &mut view, &mut flags);
        assert_eq!(scripts.errors.len(), 1, "reported once");

        let (mut scripts, dir2) = scripts_with("loop", "fn update(player, dt) { loop {} }");
        frame(&mut scripts, &mut view, &mut flags);
        assert_eq!(scripts.errors.len(), 1, "an endless loop is stopped");
        let _ = std::fs::remove_dir_all(dir);
        let _ = std::fs::remove_dir_all(dir2);
    }

    #[test]
    fn syntax_errors_have_a_line() {
        assert_eq!(syntax_error("fn update(player, dt) {}\n"), None);
        let (line, _) = syntax_error("fn update() {\n  let x = ;\n}").unwrap();
        assert_eq!(line, 2);
    }
}
