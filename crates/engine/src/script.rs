//! Scripts: small Rhai programs in the project's `scripts/` folder that
//! add abilities to the player (a double jump, a dash, a glide). A graph's
//! ABILITY node turns one on or off; while it's on, the engine calls its
//! functions every frame:
//!
//! ```rhai
//! // Called once when the ability is turned on. `this` is the script's
//! // own memory, kept while it stays on.
//! fn start() { this.used = 0; }
//!
//! // Called every frame while it's on.
//! fn update(player, dt) {
//!     if player.grounded { this.used = 0; }
//!     if player.pressed("space") && !player.grounded && !player.jumped && this.used < 1 {
//!         player.jump();
//!         this.used += 1;
//!     }
//! }
//! ```
//!
//! `player` reads the controller's state and changes its velocity; `flag`
//! and `set_flag` read and set the game's flags, so scripts and graphs talk
//! to each other. Scripts reload when their file changes. A script with an
//! error stops (the error is logged and shown) until the file is fixed.

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

/// The `player` value scripts get: shared, so their changes come back.
#[derive(Clone)]
struct PlayerHandle(Rc<RefCell<PlayerView>>);

/// The flags as scripts see them this frame, and what they set.
#[derive(Default)]
struct FlagBridge {
    flags: Flags,
    set: Vec<(String, String)>,
}

struct Loaded {
    ast: Option<AST>,
    /// The script's `this`: a map it fills as it likes.
    memory: Dynamic,
    modified: Option<SystemTime>,
    started: bool,
}

pub struct Scripts {
    engine: Engine,
    /// The project folder scripts are read from.
    dir: PathBuf,
    /// The scripts that are on, by project path.
    running: BTreeMap<String, Loaded>,
    flags: Rc<RefCell<FlagBridge>>,
    /// Errors since last taken, for the editor and the test drawer.
    pub errors: Vec<String>,
}

impl Scripts {
    pub fn new(dir: PathBuf) -> Self {
        let flags = Rc::new(RefCell::new(FlagBridge::default()));
        let mut engine = Engine::new();
        engine.set_max_operations(MAX_OPERATIONS);
        engine.on_print(|text| log::info!("script: {text}"));
        register_vec(&mut engine);
        register_player(&mut engine);
        let read = flags.clone();
        engine.register_fn("flag", move |name: &str| -> Dynamic {
            match read.borrow().flags.get(name) {
                FlagValue::Bool(b) => b.into(),
                FlagValue::Number(n) => (n as f64).into(),
                FlagValue::Text(t) => t.into(),
            }
        });
        let write = flags.clone();
        engine.register_fn("set_flag", move |name: &str, value: Dynamic| {
            let text = value.to_string();
            let mut bridge = write.borrow_mut();
            bridge.flags.set(name, &text);
            bridge.set.push((name.to_owned(), text));
        });
        Self {
            engine,
            dir,
            running: BTreeMap::new(),
            flags,
            errors: Vec::new(),
        }
    }

    /// The scripts that are on from now on: new ones start fresh, the
    /// others stop.
    pub fn sync(&mut self, enabled: &BTreeSet<String>) {
        self.running.retain(|path, _| enabled.contains(path));
        for path in enabled {
            if !self.running.contains_key(path) {
                self.running.insert(
                    path.clone(),
                    Loaded {
                        ast: None,
                        memory: Dynamic::from_map(Map::new()),
                        // Never matches a file's time: compiled on the first update.
                        modified: Some(SystemTime::UNIX_EPOCH),
                        started: false,
                    },
                );
            }
        }
    }

    /// Runs every script that is on for this frame. Their changes to the
    /// player are in `player` afterwards; flags they set are in `flags`.
    pub fn update(&mut self, player: &mut PlayerView, flags: &mut Flags, dt: f32) {
        if self.running.is_empty() {
            return;
        }
        *self.flags.borrow_mut() = FlagBridge {
            flags: flags.clone(),
            set: Vec::new(),
        };
        let handle = PlayerHandle(Rc::new(RefCell::new(std::mem::take(player))));
        let paths: Vec<String> = self.running.keys().cloned().collect();
        for path in paths {
            self.reload_if_changed(&path);
            let Some(loaded) = self.running.get_mut(&path) else {
                continue;
            };
            let Some(ast) = &loaded.ast else { continue };
            let mut result = Ok(());
            if !loaded.started {
                loaded.started = true;
                result = call(&self.engine, ast, &mut loaded.memory, "start", ());
            }
            if result.is_ok() {
                let args = (handle.clone(), dt as f64);
                result = call(&self.engine, ast, &mut loaded.memory, "update", args);
            }
            if let Err(err) = result {
                // Stopped until the file changes.
                loaded.ast = None;
                self.report(format!("{path}: {err}"));
            }
        }
        *player = handle.0.borrow().clone();
        for (name, text) in self.flags.borrow_mut().set.drain(..) {
            flags.set(&name, &text);
        }
    }

    /// Compiles a script when it's new or its file changed; it then starts
    /// again with empty memory.
    fn reload_if_changed(&mut self, path: &str) {
        let file = self.dir.join(path);
        let modified = std::fs::metadata(&file).and_then(|m| m.modified()).ok();
        let Some(loaded) = self.running.get_mut(path) else {
            return;
        };
        if loaded.modified == modified {
            return;
        }
        loaded.modified = modified;
        loaded.ast = None;
        loaded.memory = Dynamic::from_map(Map::new());
        loaded.started = false;
        let compiled = std::fs::read_to_string(&file)
            .map_err(|err| err.to_string())
            .and_then(|text| self.engine.compile(text).map_err(|err| err.to_string()));
        match compiled {
            Ok(ast) => {
                log::info!("script loaded {path}");
                loaded.ast = Some(ast);
            }
            Err(err) => self.report(format!("{path}: {err}")),
        }
    }

    fn report(&mut self, error: String) {
        log::error!("{error}");
        self.errors.push(error);
    }
}

/// Calls the script's function `name` with `this` bound to its memory;
/// a script without that function is fine.
fn call(
    engine: &Engine,
    ast: &AST,
    memory: &mut Dynamic,
    name: &str,
    args: impl rhai::FuncArgs,
) -> Result<(), Box<EvalAltResult>> {
    if !ast.iter_functions().any(|f| f.name == name) {
        return Ok(());
    }
    let options = CallFnOptions::new().eval_ast(false).bind_this_ptr(memory);
    engine
        .call_fn_with_options::<Dynamic>(options, &mut Scope::new(), ast, name, args)
        .map(|_| ())
}

/// `vec3(x, y, z)` with `.x`, `.y`, `.z`. Setters take whole numbers too,
/// so `v.y = 5` works like `v.y = 5.0`.
fn register_vec(engine: &mut Engine) {
    engine
        .register_type_with_name::<Vec3>("Vec3")
        .register_fn("vec3", |x: f64, y: f64, z: f64| {
            Vec3::new(x as f32, y as f32, z as f32)
        })
        .register_fn("to_string", |v: &mut Vec3| v.to_string())
        .register_fn("to_debug", |v: &mut Vec3| v.to_string())
        .register_fn("+", |a: Vec3, b: Vec3| a + b)
        .register_fn("-", |a: Vec3, b: Vec3| a - b)
        .register_fn("*", |a: Vec3, s: f64| a * s as f32)
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
        scripts.update(view, flags, 1.0 / 60.0);
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
            scripts.update(view, &mut flags, 0.016);
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
}
