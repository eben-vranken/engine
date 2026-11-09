//! RENAME on a scene's tab: the scene file moves, and what belongs to it or
//! points at it follows (its own graph, its bakes, open tabs, the start
//! scene in `game.ron`, LOAD SCENE and WHEN IN SCENE nodes). The file move
//! isn't an undo step; renaming it back undoes it.

use super::Editor;
use super::hierarchy::scene_name;
use super::refactor::Renamed;
use crate::graph::key;
use crate::scene::Scene;
use std::path::{Path, PathBuf};

/// The file a typed new name for the scene at `from` gives (project
/// paths). Only changing the name's case is allowed even though the file
/// "exists" (Windows doesn't tell the two apart).
pub(super) fn rename_target(
    project_dir: &Path,
    from: &Path,
    typed: &str,
) -> Result<PathBuf, String> {
    let typed = typed.trim();
    let typed = typed.strip_suffix(".ron").unwrap_or(typed);
    let stem = from
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    if typed != stem && typed.eq_ignore_ascii_case(&stem) && crate::project::is_valid_name(typed) {
        return Ok(from.with_file_name(format!("{typed}.ron")));
    }
    super::name_dialog::scene_file(project_dir, typed)
}

impl Editor {
    /// Moves the scene at `old` to `new` (project paths; `new` checked to be
    /// free) and updates everything that refers to it. `scene` is the active
    /// tab's scene.
    pub(super) fn rename_scene(
        &mut self,
        old: &Path,
        new: &Path,
        scene: &mut Scene,
    ) -> Result<(), String> {
        let dir = self.project_dir.clone();
        let case_only = key(old).eq_ignore_ascii_case(&key(new));
        let old_graph = PathBuf::from(crate::graph::scene_graph(&key(old)));
        let new_graph = PathBuf::from(crate::graph::scene_graph(&key(new)));
        let has_graph = dir.join(&old_graph).exists();
        if has_graph && !case_only && dir.join(&new_graph).exists() {
            return Err(format!("{} ALREADY EXISTS", key(&new_graph)).to_uppercase());
        }
        std::fs::rename(dir.join(old), dir.join(new))
            .map_err(|e| format!("COULD NOT RENAME {}: {e}", key(old)).to_uppercase())?;
        log::info!("renamed {} to {}", old.display(), new.display());
        // What belongs to the scene moves with it.
        let mut moves = Vec::new();
        for path_for in [crate::lightmap::path_for, crate::audio::bake::path_for] {
            moves.push((path_for(&dir.join(old)), path_for(&dir.join(new))));
        }
        if has_graph {
            moves.push((dir.join(&old_graph), dir.join(&new_graph)));
        }
        for (from, to) in moves {
            if from.exists()
                && let Err(err) = std::fs::rename(&from, &to)
            {
                self.show_error(format!("COULD NOT MOVE {}: {err}", from.display()));
                log::error!("{}: {err}", from.display());
            }
        }
        // Open documents edit the new files.
        self.repath(old, new);
        self.repath(&old_graph, &new_graph);
        // What points at it.
        let mut changed = Vec::new();
        if let Some(project) = &mut self.project {
            match project.rename_scene(old, new, &dir) {
                Ok(true) => changed.push("GAME.RON".to_owned()),
                Ok(false) => {}
                Err(err) => self.show_error(err),
            }
        }
        changed.extend(self.update_references(Renamed::Scene, &key(old), &key(new), scene, None));
        self.index.refresh(&dir, true);
        let mut text = format!("RENAMED {} TO {}", scene_name(old), scene_name(new));
        if !changed.is_empty() {
            text.push_str(&format!("; UPDATED {}", changed.join(", ").to_uppercase()));
        }
        self.show_message(text);
        Ok(())
    }

    /// Every open document at `from` is at `to` from now on.
    fn repath(&mut self, from: &Path, to: &Path) {
        let paths = std::iter::once(&mut self.scene_path)
            .chain(std::iter::once(&mut self.logic_path))
            .chain(
                self.tabs
                    .iter_mut()
                    .flatten()
                    .map(|t| &mut t.doc.scene_path),
            )
            .chain(self.logic_tabs.iter_mut().flatten().map(|t| &mut t.path));
        for path in paths {
            if *path == from {
                *path = to.to_owned();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rename_target_may_only_change_case_of_its_own_name() {
        let sandbox = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sandbox");
        let from = Path::new("scenes/test_level.ron");
        assert_eq!(
            rename_target(&sandbox, from, "Test_Level").unwrap(),
            Path::new("scenes/Test_Level.ron")
        );
        assert_eq!(
            rename_target(&sandbox, from, "hall").unwrap(),
            Path::new("scenes").join("hall.ron")
        );
        assert!(rename_target(&sandbox, from, "../x").is_err());
    }

    /// The scene, its graph and bakes move; the tab, the start scene and a
    /// LOAD SCENE node in another graph follow.
    #[test]
    fn renaming_a_scene_moves_its_files_and_updates_references() {
        use crate::graph::{Graph, NodeKind};
        let dir = std::env::temp_dir().join("engine-rename-scene");
        let _ = std::fs::remove_dir_all(&dir);
        crate::project::create(&dir).unwrap();
        std::fs::create_dir_all(dir.join("graphs/scenes")).unwrap();
        Graph::starter()
            .save(&dir.join("graphs/scenes/main.ron"))
            .unwrap();
        let mut door = Graph::starter();
        door.add(
            NodeKind::LoadScene {
                scene: crate::scene::SceneRef("scenes/main.ron".into()),
            },
            glam::Vec2::ZERO,
        );
        door.save(&dir.join("graphs/door.ron")).unwrap();
        std::fs::write(dir.join("scenes/main.lightmap"), b"bake").unwrap();

        let mut editor = Editor::new("test".into(), dir.clone(), "scenes/main.ron".into());
        let mut scene = Scene::load(&dir.join("scenes/main.ron")).unwrap();
        editor
            .rename_scene(
                Path::new("scenes/main.ron"),
                Path::new("scenes/hall.ron"),
                &mut scene,
            )
            .unwrap();

        assert!(!dir.join("scenes/main.ron").exists());
        assert!(dir.join("scenes/hall.ron").exists());
        assert!(dir.join("scenes/hall.lightmap").exists());
        assert!(dir.join("graphs/scenes/hall.ron").exists());
        assert_eq!(editor.scene_path, Path::new("scenes/hall.ron"));
        let project = crate::project::Project::open(&dir).unwrap();
        assert_eq!(project.settings.start_scene, Path::new("scenes/hall.ron"));
        let door = Graph::load(&dir.join("graphs/door.ron")).unwrap();
        assert!(door.nodes.iter().any(|n| matches!(
            &n.kind,
            NodeKind::LoadScene { scene } if scene.0 == "scenes/hall.ron"
        )));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
