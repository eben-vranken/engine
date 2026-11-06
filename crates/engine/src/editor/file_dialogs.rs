//! Everything that goes through the system's file dialog: NEW PROJECT,
//! OPEN PROJECT, OPEN PACKAGE, EXPORT PACKAGE and SAVE SCENE AS. The dialog
//! runs on a thread of its own so the editor keeps drawing; its answer is
//! picked up in a later frame. Switching projects asks about unsaved
//! changes first; the runtime then opens the editor again on the other
//! project.

use super::confirm::Confirm;
use super::{Editor, EditorOutput};
use crate::player::settings::PlayerSettings;
use crate::scene::Scene;
use crate::{package, project};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};

/// What a file dialog is open for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Purpose {
    NewProject,
    OpenProject,
    /// Pick a package file…
    OpenPackage,
    /// …then the folder to unpack it into.
    UnpackInto(PathBuf),
    ExportPackage,
    SaveSceneAs,
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
        let project = std::path::absolute(&self.project_dir).unwrap_or_default();
        let beside_project = project.parent().map(Path::to_path_buf).unwrap_or_default();
        let dialog = rfd::FileDialog::new();
        let dialog = match &purpose {
            Purpose::NewProject => dialog
                .set_title("New project: name the game's folder")
                .set_directory(&beside_project)
                .set_file_name("new game"),
            Purpose::OpenProject => dialog
                .set_title("Open project: pick its game.ron")
                .set_directory(&beside_project)
                .add_filter("Game project", &["ron"]),
            Purpose::OpenPackage => dialog
                .set_title("Open package")
                .set_directory(&beside_project)
                .add_filter("Game package", &[package::EXTENSION]),
            Purpose::UnpackInto(file) => dialog
                .set_title("Unpack the package into…")
                .set_directory(file.parent().unwrap_or(&beside_project)),
            Purpose::ExportPackage => {
                let out = package::export_path(&self.project_dir);
                dialog
                    .set_title("Export package")
                    .set_directory(out.parent().unwrap_or(&beside_project))
                    .set_file_name(out.file_name().unwrap_or_default().to_string_lossy())
                    .add_filter("Game package", &[package::EXTENSION])
            }
            Purpose::SaveSceneAs => dialog
                .set_title("Save scene as")
                .set_directory(project.join("scenes"))
                .set_file_name(
                    self.scene_path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy(),
                )
                .add_filter("Scene", &["ron"]),
        };
        let saving = matches!(
            purpose,
            Purpose::NewProject | Purpose::ExportPackage | Purpose::SaveSceneAs
        );
        let picking_folder = matches!(purpose, Purpose::UnpackInto(_));
        let (send, answer) = mpsc::channel();
        std::thread::spawn(move || {
            let picked = if saving {
                dialog.save_file()
            } else if picking_folder {
                dialog.pick_folder()
            } else {
                dialog.pick_file()
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
    pub(super) fn poll_file_dialog(
        &mut self,
        scene: &Scene,
        player: &PlayerSettings,
        output: &mut EditorOutput,
    ) {
        let Some(dialog) = &self.file_dialog else {
            return;
        };
        let picked = match dialog.answer.try_recv() {
            Ok(picked) => picked,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => None,
        };
        let purpose = dialog.purpose.clone();
        self.file_dialog = None;
        let Some(path) = picked else {
            self.message = None; // cancelled
            return;
        };
        let result = match purpose {
            Purpose::NewProject => check_new(&path).map(|dir| self.switch(Switch::Create(dir))),
            Purpose::OpenProject => project::find(&path)
                .map_err(|e| e.to_string())
                .and_then(|dir| self.check_other(dir))
                .map(|dir| self.switch(Switch::Open(dir))),
            Purpose::OpenPackage => {
                self.ask_file(Purpose::UnpackInto(path));
                Ok(None)
            }
            Purpose::UnpackInto(file) => {
                let target = package::unpack_target(&file, &path);
                package::unpack_into(&file, &target)
                    .map_err(|e| e.to_string())
                    .map(|_| self.switch(Switch::Open(target)))
            }
            Purpose::ExportPackage => {
                output.export_package = Some(with_extension(path, package::EXTENSION));
                Ok(None)
            }
            Purpose::SaveSceneAs => scene_in_project(&self.project_dir, &path)
                .and_then(|relative| self.save_scene_as(&relative, scene, player, output))
                .map(|()| None),
        };
        match result {
            Ok(Some(switch)) => self.switch_project(switch, output),
            Ok(None) => {}
            Err(err) => {
                log::error!("{err}");
                self.show_error(err);
            }
        }
    }

    /// Asks about unsaved changes before a switch. Returns the switch if it
    /// can happen right away.
    pub(super) fn switch(&mut self, switch: Switch) -> Option<Switch> {
        if self.any_dirty() {
            self.confirm = Some(Confirm::Project(switch));
            None
        } else {
            Some(switch)
        }
    }

    /// A package was dropped on the editor: unpacks it next to itself and
    /// switches to it (after unsaved changes). Returns the folder to open
    /// now, if any.
    pub fn open_dropped_package(&mut self, file: &Path) -> Option<PathBuf> {
        match package::unpack(file) {
            Ok((dir, _)) => match self.switch(Switch::Open(dir)) {
                Some(Switch::Open(dir)) => Some(dir),
                _ => None,
            },
            Err(err) => {
                log::error!("{err}");
                self.show_error(err.to_string());
                None
            }
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
        if same_place(&dir, &self.project_dir) {
            Err("THAT PROJECT IS ALREADY OPEN".into())
        } else {
            Ok(dir)
        }
    }

    /// SAVE SCENE AS to `path` (relative to the project, in `scenes/`).
    fn save_scene_as(
        &mut self,
        path: &Path,
        scene: &Scene,
        player: &PlayerSettings,
        output: &mut EditorOutput,
    ) -> Result<(), String> {
        if path == self.scene_path {
            output.settled |= self.save(scene, player);
            return Ok(());
        }
        if self.tab_of(path).is_some() {
            return Err(format!("{} IS OPEN IN ANOTHER TAB", path.display()));
        }
        self.save_as(path, scene, player, output)
    }
}

/// Whether two paths are the same file or folder on disk.
fn same_place(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// `path` with `.extension` added unless it already has it.
fn with_extension(path: PathBuf, extension: &str) -> PathBuf {
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(extension))
    {
        path
    } else {
        let mut name = path.into_os_string();
        name.push(format!(".{extension}"));
        name.into()
    }
}

/// A scene file picked in the save dialog, relative to the project. It
/// must be in the project's `scenes/` folder and have a valid name.
fn scene_in_project(project_dir: &Path, picked: &Path) -> Result<PathBuf, String> {
    let picked = with_extension(picked.to_owned(), "ron");
    let folder = picked.parent().unwrap_or(Path::new(""));
    if !same_place(folder, &project_dir.join("scenes")) {
        return Err("SCENES GO IN THE PROJECT'S SCENES/ FOLDER".into());
    }
    let name = picked.file_stem().unwrap_or_default().to_string_lossy();
    if !project::is_valid_name(&name) {
        return Err(project::ProjectError::BadName(name.into_owned()).to_string());
    }
    Ok(Path::new("scenes").join(picked.file_name().unwrap_or_default()))
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

    fn sandbox() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sandbox")
    }

    #[test]
    fn a_new_project_needs_a_good_name_and_room() {
        let sandbox = sandbox();
        assert!(check_new(&sandbox.join("..").join("brand new game")).is_ok());
        assert!(check_new(&sandbox).is_err(), "not empty");
        assert!(check_new(&sandbox.join("game.ron")).is_err(), "a file");
        assert!(check_new(&sandbox.join("..").join("what?")).is_err());
    }

    #[test]
    fn opening_the_open_project_is_refused() {
        let sandbox = sandbox();
        let editor = Editor::new("test".into(), sandbox.clone(), PathBuf::new());
        assert!(editor.check_other(sandbox.join("scenes/..")).is_err());
        assert!(editor.check_other(sandbox.join("scenes")).is_ok());
    }

    #[test]
    fn saved_scenes_stay_in_the_projects_scenes_folder() {
        let sandbox = sandbox();
        let scenes = sandbox.join("scenes");
        assert_eq!(
            scene_in_project(&sandbox, &scenes.join("cellar")).unwrap(),
            Path::new("scenes/cellar.ron")
        );
        assert_eq!(
            scene_in_project(&sandbox, &scenes.join("cellar.ron")).unwrap(),
            Path::new("scenes/cellar.ron")
        );
        assert!(scene_in_project(&sandbox, &sandbox.join("cellar.ron")).is_err());
        assert!(scene_in_project(&sandbox, &sandbox.join("../cellar.ron")).is_err());
        assert!(scene_in_project(&sandbox, &scenes.join("a?b.ron")).is_err());
    }

    #[test]
    fn exports_get_the_package_extension() {
        assert_eq!(
            with_extension("out/game".into(), "gamepack"),
            Path::new("out/game.gamepack")
        );
        assert_eq!(
            with_extension("out/game.GAMEPACK".into(), "gamepack"),
            Path::new("out/game.GAMEPACK")
        );
    }
}
