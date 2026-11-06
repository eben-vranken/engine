//! NEW PROJECT and OPEN PROJECT. The system's file dialog runs on a thread
//! of its own so the editor keeps drawing; its answer is picked up in a
//! later frame. Unsaved changes are asked about before the switch, which
//! the runtime does by opening the editor again on the other project.

use super::confirm::Confirm;
use super::{Editor, EditorOutput};
use crate::project;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};

/// What a file dialog was opened for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Purpose {
    NewProject,
    OpenProject,
}

/// A file dialog that is open.
pub(super) struct FileDialog {
    purpose: Purpose,
    answer: Receiver<Option<PathBuf>>,
}

/// A project switch waiting for its turn (after unsaved changes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Switch {
    /// Create a new project in this folder, then open it.
    Create(PathBuf),
    Open(PathBuf),
}

impl Editor {
    /// Opens the system's file dialog for `purpose`, unless one is open.
    pub(super) fn ask_file(&mut self, purpose: Purpose) {
        if self.file_dialog.is_some() {
            return;
        }
        // Start next to the current project.
        let near = self
            .project_dir
            .canonicalize()
            .ok()
            .and_then(|dir| dir.parent().map(Path::to_path_buf))
            .unwrap_or_default();
        let dialog = rfd::FileDialog::new().set_directory(near);
        let (send, answer) = mpsc::channel();
        std::thread::spawn(move || {
            let picked = match purpose {
                Purpose::NewProject => dialog
                    .set_title("New project: name the game's folder")
                    .set_file_name("new game")
                    .save_file(),
                Purpose::OpenProject => dialog
                    .set_title("Open project: pick its game.ron")
                    .add_filter("Game project", &["ron"])
                    .pick_file(),
            };
            let _ = send.send(picked);
        });
        self.file_dialog = Some(FileDialog { purpose, answer });
        self.show_message("WAITING FOR THE FILE DIALOG".into());
    }

    /// Whether a file dialog is open (the menus that open one wait).
    pub(super) fn file_dialog_open(&self) -> bool {
        self.file_dialog.is_some()
    }

    /// Acts on the file dialog's answer once there is one.
    pub(super) fn poll_file_dialog(&mut self, output: &mut EditorOutput) {
        let Some(dialog) = &self.file_dialog else {
            return;
        };
        let picked = match dialog.answer.try_recv() {
            Ok(picked) => picked,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => None,
        };
        let purpose = dialog.purpose;
        self.file_dialog = None;
        let Some(path) = picked else {
            self.message = None; // cancelled
            return;
        };
        let switch = match purpose {
            Purpose::NewProject => check_new(&path).map(Switch::Create),
            Purpose::OpenProject => project::find(&path)
                .map_err(|e| e.to_string())
                .and_then(|dir| self.check_other(dir))
                .map(Switch::Open),
        };
        match switch {
            Ok(switch) if self.any_dirty() => self.confirm = Some(Confirm::Project(switch)),
            Ok(switch) => self.switch_project(switch, output),
            Err(err) => self.show_error(err),
        }
    }

    /// Makes the switch (unsaved changes are dealt with).
    pub(super) fn switch_project(&mut self, switch: Switch, output: &mut EditorOutput) {
        let dir = match switch {
            Switch::Create(dir) => match project::create(&dir) {
                Ok(_) => {
                    log::info!("created the project {}", dir.display());
                    dir
                }
                Err(err) => {
                    log::error!("{err}");
                    return self.show_error(err.to_string());
                }
            },
            Switch::Open(dir) => dir,
        };
        self.show_message(format!("OPENING {}", dir.display()));
        output.open_project = Some(dir);
    }

    /// `dir`, unless it is the project already open.
    fn check_other(&self, dir: PathBuf) -> Result<PathBuf, String> {
        let same = |a: &Path, b: &Path| match (a.canonicalize(), b.canonicalize()) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        };
        if same(&dir, &self.project_dir) {
            Err("THAT PROJECT IS ALREADY OPEN".into())
        } else {
            Ok(dir)
        }
    }
}

/// Checks the folder a new project would be made in.
fn check_new(dir: &Path) -> Result<PathBuf, String> {
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if !project::is_valid_name(&name) {
        return Err(project::ProjectError::BadName(name).to_string());
    }
    if dir.is_file() {
        return Err(format!("{} IS A FILE", dir.display()));
    }
    if dir
        .read_dir()
        .is_ok_and(|mut entries| entries.next().is_some())
    {
        return Err(project::ProjectError::NotEmpty(dir.to_owned()).to_string());
    }
    Ok(dir.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_project_needs_a_good_name_and_room() {
        let sandbox = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sandbox");
        assert!(check_new(&sandbox.join("..").join("brand new game")).is_ok());
        assert!(check_new(&sandbox).is_err(), "not empty");
        assert!(check_new(&sandbox.join("game.ron")).is_err(), "a file");
        assert!(check_new(&sandbox.join("..").join("what?")).is_err());
    }

    #[test]
    fn opening_the_open_project_is_refused() {
        let sandbox = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sandbox");
        let editor = Editor::new("test".into(), sandbox.clone(), PathBuf::new());
        assert!(editor.check_other(sandbox.join("scenes/..")).is_err());
        assert!(editor.check_other(sandbox.join("scenes")).is_ok());
    }
}
