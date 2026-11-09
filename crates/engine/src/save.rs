//! Saved games: where the player is, the graphs (flags, active nodes,
//! renamed characters) and what changed in each scene (objects hidden or
//! shown, doors open) plus the zones visited. One save per project, kept in
//! the user's config folder (never in the project).

use crate::graph::run::{RunnerState, ThreadState};
use glam::Vec3;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SaveGame {
    /// The scene the player was in, as a project path with `/`.
    pub scene: String,
    /// The player's feet and facing (radians).
    pub position: Vec3,
    pub yaw: f32,
    pub graphs: RunnerState,
    pub world: WorldState,
}

/// What the player changed in the world, kept across scenes.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct WorldState {
    /// Per scene (project path with `/`).
    pub scenes: BTreeMap<String, SceneState>,
    /// Zones visited, per scene: the map shows these.
    pub visited: BTreeMap<String, BTreeSet<String>>,
    /// Scene graphs paused while the player is in another scene.
    pub scene_graphs: BTreeMap<String, Vec<ThreadState>>,
}

/// One scene's changed objects.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SceneState {
    /// Per object, by its id (stable through renames).
    pub objects: BTreeMap<u32, ObjectState>,
    /// Saves from before object ids, by name: read, never written.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub hidden: BTreeMap<String, bool>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub doors: BTreeMap<String, bool>,
}

/// What the player changed about one object.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ObjectState {
    /// Whether it is hidden (picked up, hidden or shown by a graph).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
    /// Whether the door is open.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub door_open: Option<bool>,
}

impl WorldState {
    pub fn visit(&mut self, scene: &str, zone: &str) -> bool {
        self.visited
            .entry(scene.to_owned())
            .or_default()
            .insert(zone.to_owned())
    }
}

/// The save file for the project in `project_dir`; `None` without a config
/// folder. Named after the project folder plus a short hash of its full
/// path, so two projects with the same name don't share a save.
pub fn file(project_dir: &Path) -> Option<PathBuf> {
    let absolute = std::fs::canonicalize(project_dir).unwrap_or_else(|_| project_dir.to_owned());
    let name = absolute
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "game".into());
    let hash = fnv(absolute.to_string_lossy().as_bytes());
    Some(
        crate::recent::config_dir()?
            .join("saves")
            .join(format!("{name}-{:08x}.ron", hash as u32)),
    )
}

/// FNV-1a: stable across Rust versions, unlike the standard hasher.
fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |h, &b| {
        (h ^ b as u64).wrapping_mul(0x100000001b3)
    })
}

/// The project's saved game, if there is one that reads.
pub fn load(project_dir: &Path) -> Option<SaveGame> {
    let path = file(project_dir)?;
    let text = std::fs::read_to_string(&path).ok()?;
    ron::from_str(&text)
        .inspect_err(|err| log::error!("{}: {err}", path.display()))
        .ok()
}

pub fn exists(project_dir: &Path) -> bool {
    file(project_dir).is_some_and(|f| f.is_file())
}

pub fn write(project_dir: &Path, save: &SaveGame) -> Result<PathBuf, String> {
    let path = file(project_dir).ok_or("no config folder to save in")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let config = ron::ser::PrettyConfig::default().struct_names(false);
    let text = ron::ser::to_string_pretty(save, config).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_read_back() {
        let mut save = SaveGame {
            scene: "scenes/hall.ron".into(),
            position: Vec3::new(1.0, 0.0, -2.0),
            yaw: 0.5,
            ..Default::default()
        };
        save.graphs.flags.set("lantern", "true");
        save.world.visit("scenes/hall.ron", "hall");
        save.world
            .scenes
            .entry("scenes/hall.ron".into())
            .or_default()
            .doors
            .insert("gate".into(), true);
        let config = ron::ser::PrettyConfig::default().struct_names(false);
        let text = ron::ser::to_string_pretty(&save, config).unwrap();
        assert_eq!(ron::from_str::<SaveGame>(&text).unwrap(), save);
    }

    #[test]
    fn projects_get_their_own_save() {
        if crate::recent::config_dir().is_none() {
            return;
        }
        let a = file(Path::new("one/game")).unwrap();
        let b = file(Path::new("two/game")).unwrap();
        assert_ne!(a, b);
        assert!(
            a.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("game-")
        );
    }
}

#[cfg(test)]
mod scene_state_tests {
    use super::*;

    /// Old saves (by name) still load; new ones are by id.
    #[test]
    fn scene_state_reads_old_saves_and_writes_ids() {
        let old: SceneState =
            ron::from_str("(hidden: {\"feather\": true}, doors: {\"gate\": true})").unwrap();
        assert_eq!(old.hidden.get("feather"), Some(&true));
        assert_eq!(old.doors.get("gate"), Some(&true));

        let mut new = SceneState::default();
        new.objects.entry(7).or_default().hidden = Some(true);
        let text = ron::to_string(&new).unwrap();
        assert!(!text.contains("doors"), "{text}");
        assert_eq!(ron::from_str::<SceneState>(&text).unwrap(), new);
    }
}
