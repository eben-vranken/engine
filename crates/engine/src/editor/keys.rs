//! Every key, in one table: what it does and where. The editor runs the
//! commands from it (the same commands as the menus and CTRL+K), and the
//! `?` sheet and the keys shown next to commands are made from it, so they
//! can't disagree.

use super::Editor;
use super::commands::{Command, GizmoTool, Workspace};
use crate::input::Input;
use winit::keyboard::KeyCode;

/// Where a key means what it says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Context {
    Everywhere,
    Build,
    Logic,
    Play,
}

impl Context {
    pub(super) const ALL: [Context; 4] = [
        Context::Everywhere,
        Context::Build,
        Context::Logic,
        Context::Play,
    ];

    pub(super) fn title(self) -> &'static str {
        match self {
            Context::Everywhere => "EVERYWHERE",
            Context::Build => "BUILD",
            Context::Logic => "LOGIC",
            Context::Play => "PLAYING",
        }
    }
}

/// What a key does.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Action {
    /// An editor command, run like a menu item.
    Run(Command),
    /// Handled by the game or a widget; listed here so the sheet is whole.
    Note(&'static str),
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct Binding {
    /// As shown: `CTRL+S`, `W E R`.
    pub keys: &'static str,
    /// The key that runs it (`None`: a note only).
    pub key: Option<KeyCode>,
    pub ctrl: bool,
    pub shift: bool,
    pub context: Context,
    pub action: Action,
    /// Not while flying with the right mouse button (WASD, Q, E move).
    pub not_flying: bool,
}

fn run(keys: &'static str, key: KeyCode, context: Context, command: Command) -> Binding {
    let ctrl = keys.starts_with("CTRL+");
    let shift = keys.contains("SHIFT+");
    Binding {
        keys,
        key: Some(key),
        ctrl,
        shift,
        context,
        action: Action::Run(command),
        not_flying: false,
    }
}

fn note(keys: &'static str, context: Context, what: &'static str) -> Binding {
    Binding {
        keys,
        key: None,
        ctrl: false,
        shift: false,
        context,
        action: Action::Note(what),
        not_flying: false,
    }
}

/// A key the runtime catches before the UI does (Tab, which egui would use
/// to move focus); listed with its command.
fn early(keys: &'static str, context: Context, command: Command) -> Binding {
    Binding {
        action: Action::Run(command),
        ..note(keys, context, "")
    }
}

fn grounded(mut binding: Binding) -> Binding {
    binding.not_flying = true;
    binding
}

/// The table.
pub(super) fn bindings() -> Vec<Binding> {
    use Context::*;
    vec![
        run("CTRL+K", KeyCode::KeyK, Everywhere, Command::Search),
        run("CTRL+S", KeyCode::KeyS, Everywhere, Command::Save),
        run("CTRL+SHIFT+S", KeyCode::KeyS, Everywhere, Command::SaveAll),
        run("CTRL+Z", KeyCode::KeyZ, Everywhere, Command::Undo),
        run("CTRL+Y", KeyCode::KeyY, Everywhere, Command::Redo),
        run("CTRL+SHIFT+Z", KeyCode::KeyZ, Everywhere, Command::Redo),
        note("F5", Everywhere, "PLAY THE OPEN SCENE / STOP"),
        note("SHIFT+F5", Everywhere, "PLAY IN A WINDOW, AS A PLAYER"),
        early("TAB", Everywhere, Command::ToggleLeft),
        early("SHIFT+TAB", Everywhere, Command::ToggleRight),
        note("?", Everywhere, "THIS LIST"),
        note("[ ] 0", Everywhere, "RENDER SCALE DOWN / UP / AUTO"),
        note("V", Everywhere, "VSYNC"),
        note(
            "CLICK / SHIFT+CLICK",
            Build,
            "SELECT / ADD TO THE SELECTION",
        ),
        grounded(run(
            "W",
            KeyCode::KeyW,
            Build,
            Command::Gizmo(GizmoTool::Move),
        )),
        grounded(run(
            "E",
            KeyCode::KeyE,
            Build,
            Command::Gizmo(GizmoTool::Rotate),
        )),
        grounded(run(
            "R",
            KeyCode::KeyR,
            Build,
            Command::Gizmo(GizmoTool::Scale),
        )),
        note("CTRL (DRAGGING)", Build, "SNAP"),
        note(
            "HOLD RMB + WASD, Q/E",
            Build,
            "FLY; SHIFT FAST, WHEEL SPEED",
        ),
        grounded(run("CTRL+D", KeyCode::KeyD, Build, Command::Duplicate)),
        run("DEL", KeyCode::Delete, Build, Command::Delete),
        run("ESC", KeyCode::Escape, Build, Command::Deselect),
        grounded(run("H", KeyCode::KeyH, Build, Command::ToggleZones)),
        note("SPACE", Logic, "ADD A NODE"),
        run("DEL", KeyCode::Delete, Logic, Command::Delete),
        run("CTRL+D", KeyCode::KeyD, Logic, Command::Duplicate),
        note("DRAG A RIGHT DOT", Logic, "CONNECT TO ANOTHER NODE"),
        note("WHEEL / DRAG BACKGROUND", Logic, "ZOOM / PAN"),
        note("WASD SPACE SHIFT CTRL", Play, "MOVE, JUMP, SPRINT, CROUCH"),
        note("E", Play, "USE, TALK, TAKE"),
        note("M", Play, "MAP"),
        note("ESC", Play, "FREE THE MOUSE"),
        note("F1", Play, "PLAYER FEEL (THE TEST DRAWER'S FEEL TAB)"),
        note("F2", Play, "FREE CAMERA"),
        note(
            "F3",
            Play,
            "TEST DRAWER (LOGIC, FLAGS, AUDIO, FEEL, MAP, STATS)",
        ),
        note("F6 / F7", Play, "PAUSE / STEP A FRAME (EDITOR)"),
        note("Q", Play, "CLAP"),
        note("R", Play, "RESPAWN"),
        note("W/S, 1–9, E", Play, "DIALOGUE: CHOOSE, PICK, GO ON"),
    ]
}

/// The keys of a command, as shown next to it (the first binding).
pub(super) fn keys_of(command: &Command) -> &'static str {
    bindings()
        .into_iter()
        .find(|b| b.action == Action::Run(command.clone()))
        .map_or("", |b| b.keys)
}

impl Editor {
    /// Queues the commands whose keys were pressed this frame, in the
    /// context on screen; they run with the menus' rules (`can_run`).
    /// `flying`: the right mouse button flies the camera.
    pub fn press_keys(&mut self, input: &Input, flying: bool) {
        let ctrl = input.held(KeyCode::ControlLeft) || input.held(KeyCode::ControlRight);
        let shift = input.held(KeyCode::ShiftLeft) || input.held(KeyCode::ShiftRight);
        let here = match self.workspace() {
            Workspace::Build => Some(Context::Build),
            Workspace::Logic if self.graph.is_some() => Some(Context::Logic),
            _ => None,
        };
        for binding in bindings() {
            let (Some(key), Action::Run(command)) = (binding.key, binding.action) else {
                continue;
            };
            let applies = binding.context == Context::Everywhere || Some(binding.context) == here;
            if applies
                && input.pressed(key)
                && binding.ctrl == ctrl
                && binding.shift == shift
                && !(flying && binding.not_flying)
            {
                self.queued.push(command);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One meaning per key and context.
    #[test]
    fn no_key_means_two_things_in_one_place() {
        let table = bindings();
        for (i, a) in table.iter().enumerate() {
            for b in &table[i + 1..] {
                let same_key =
                    a.key.is_some() && a.key == b.key && a.ctrl == b.ctrl && a.shift == b.shift;
                let overlap = a.context == b.context
                    || a.context == Context::Everywhere
                    || b.context == Context::Everywhere;
                assert!(
                    !(same_key && overlap),
                    "{} in {:?} and {:?}",
                    a.keys,
                    a.context,
                    b.context
                );
            }
        }
    }
}
