//! A game project: a content-only folder with `game.ron` at its root.

use crate::scene::{Scene, SceneError};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// `game.ron`: settings for the whole game.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GameSettings {
    pub name: String,
    /// Scene loaded at start, relative to the project folder.
    pub start_scene: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    #[error("{0} is not a game project (no game.ron): {1}")]
    Read(PathBuf, std::io::Error),
    #[error("{path}: {source}")]
    Parse {
        path: PathBuf,
        source: Box<ron::error::SpannedError>,
    },
    #[error(transparent)]
    Scene(#[from] SceneError),
}

pub struct Project {
    pub dir: PathBuf,
    pub settings: GameSettings,
}

impl Project {
    pub fn open(dir: &Path) -> Result<Self, ProjectError> {
        let path = dir.join("game.ron");
        let text = std::fs::read_to_string(&path)
            .map_err(|err| ProjectError::Read(dir.to_owned(), err))?;
        let settings = ron::from_str(&text).map_err(|source| ProjectError::Parse {
            path,
            source: Box::new(source),
        })?;
        Ok(Self {
            dir: dir.to_owned(),
            settings,
        })
    }

    pub fn start_scene_path(&self) -> PathBuf {
        self.dir.join(&self.settings.start_scene)
    }

    pub fn load_start_scene(&self) -> Result<Scene, ProjectError> {
        Ok(Scene::load(&self.start_scene_path())?)
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    /// The sandbox in this repo must always open and its start scene load.
    #[test]
    fn sandbox_opens() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sandbox");
        let project = super::Project::open(&dir).expect("sandbox/game.ron");
        let scene = project.load_start_scene().expect("sandbox start scene");
        assert!(
            scene.player_start().is_some(),
            "start scene needs a PlayerStart"
        );
    }
}
