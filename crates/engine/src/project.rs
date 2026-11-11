//! A game project: a content-only folder with `game.ron` at its root.

use crate::dialogue::DialogueStyle;
use crate::scene::{Scene, SceneError};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// `game.ron`: settings for the whole game.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct GameSettings {
    pub name: String,
    /// Scene loaded at start, relative to the project folder.
    pub start_scene: PathBuf,
    /// The render scale the game starts at: output pixels per internal
    /// pixel (the `[` / `]` keys still change it). `None` = automatic
    /// (internal height about 720 lines or less).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub render_scale: Option<u32>,
    /// How dialogue looks.
    #[serde(default, skip_serializing_if = "DialogueStyle::is_default")]
    pub dialogue: DialogueStyle,
    /// Old-console looks, all off unless the game turns them on.
    #[serde(default, skip_serializing_if = "Retro::is_default")]
    pub retro: Retro,
}

/// Optional looks of early 3D consoles, per game.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, bevy_reflect::Reflect)]
#[serde(default)]
pub struct Retro {
    /// Vertices land on a coarse screen grid, so edges wobble as things
    /// move.
    pub vertex_snap: bool,
    /// Lines (down the screen) of that grid; fewer = more wobble.
    pub snap_lines: u32,
    /// Textures are drawn without perspective correction, so they warp
    /// on big surfaces seen at an angle.
    pub affine: bool,
    /// Few color levels per channel, with a dither pattern in between.
    pub dither: bool,
    /// Color levels per channel when dithering (32 = 15-bit color).
    pub dither_levels: u32,
}

impl Default for Retro {
    fn default() -> Self {
        Self {
            vertex_snap: false,
            snap_lines: 240,
            affine: false,
            dither: false,
            dither_levels: 32,
        }
    }
}

impl Retro {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

impl GameSettings {
    /// Writes `game.ron` in the project folder `dir`.
    pub fn save(&self, dir: &Path) -> Result<(), ProjectError> {
        let path = dir.join("game.ron");
        let text = ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default())
            .expect("game settings always serialize");
        std::fs::write(&path, text).map_err(|err| ProjectError::Io(path, err))
    }
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
    #[error("{0} has no game.ron, so it isn't a game project")]
    NotAProject(PathBuf),
    #[error("{0} already exists and isn't empty")]
    NotEmpty(PathBuf),
    #[error("\"{0}\" can't be a project name: use letters, digits, spaces, - and _ only")]
    BadName(String),
    #[error("{0}: {1}")]
    Io(PathBuf, std::io::Error),
}

/// Whether `name` is usable for a project, scene or model: letters,
/// digits, spaces, `-` and `_`, not blank.
pub fn is_valid_name(name: &str) -> bool {
    !name.trim().is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | ' '))
}

/// The project a picked path belongs to: a folder with `game.ron` in it,
/// or that `game.ron` itself.
pub fn find(path: &Path) -> Result<PathBuf, ProjectError> {
    let dir = if path.file_name().is_some_and(|n| n == "game.ron") {
        path.parent().unwrap_or(path)
    } else {
        path
    };
    if dir.join("game.ron").is_file() {
        Ok(dir.to_owned())
    } else {
        Err(ProjectError::NotAProject(path.to_owned()))
    }
}

/// Makes a new, content-only game project in `dir` (which must not exist
/// yet, or be empty), named after the folder: `game.ron`, a starter
/// `scenes/main.ron`, and empty `models/`, `textures/` and `audio/`.
pub fn create(dir: &Path) -> Result<GameSettings, ProjectError> {
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if !is_valid_name(&name) {
        return Err(ProjectError::BadName(name));
    }
    let io = |path: &Path| {
        let path = path.to_owned();
        move |err| ProjectError::Io(path, err)
    };
    if dir.exists() && std::fs::read_dir(dir).map_err(io(dir))?.next().is_some() {
        return Err(ProjectError::NotEmpty(dir.to_owned()));
    }
    for folder in ["scenes", "models", "textures", "audio"] {
        let path = dir.join(folder);
        std::fs::create_dir_all(&path).map_err(io(&path))?;
    }
    let settings = GameSettings {
        name: name.trim().to_owned(),
        start_scene: PathBuf::from("scenes/main.ron"),
        render_scale: None,
        dialogue: DialogueStyle::default(),
        retro: Retro::default(),
    };
    Scene::starter().save(&dir.join(&settings.start_scene))?;
    settings.save(dir)?;
    Ok(settings)
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
    use super::*;

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

    /// A fresh folder in the system temp folder, removed when dropped.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("engine-test-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_new_project_opens_and_plays() {
        let temp = TempDir::new("create");
        let dir = temp.0.join("My Game");
        let settings = create(&dir).unwrap();
        assert_eq!(settings.name, "My Game");
        for folder in ["models", "textures", "audio"] {
            assert!(dir.join(folder).is_dir(), "{folder}/");
        }
        assert!(!dir.join("player.ron").exists());
        let project = Project::open(&dir).unwrap();
        assert_eq!(project.settings, settings);
        assert!(project.load_start_scene().unwrap().player_start().is_some());

        // Not over an existing project, and not with a bad name.
        assert!(matches!(create(&dir), Err(ProjectError::NotEmpty(_))));
        assert!(matches!(
            create(&temp.0.join("a/b?")),
            Err(ProjectError::BadName(_))
        ));
        // An empty folder is fine.
        let empty = temp.0.join("empty");
        std::fs::create_dir(&empty).unwrap();
        assert!(create(&empty).is_ok());
    }

    #[test]
    fn a_project_is_found_from_its_folder_or_its_game_ron() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sandbox");
        assert_eq!(find(&dir).unwrap(), dir);
        assert_eq!(find(&dir.join("game.ron")).unwrap(), dir);
        assert!(matches!(
            find(&dir.join("scenes")),
            Err(ProjectError::NotAProject(_))
        ));
    }

    #[test]
    fn settings_save_and_load_with_a_render_scale() {
        let temp = TempDir::new("settings");
        let dir = temp.0.join("game");
        let mut settings = create(&dir).unwrap();
        assert_eq!(settings.render_scale, None);
        let text = std::fs::read_to_string(dir.join("game.ron")).unwrap();
        assert!(!text.contains("render_scale"), "auto isn't written");
        settings.retro.affine = true;
        settings.render_scale = Some(5);
        settings.name = "Renamed".into();
        settings.save(&dir).unwrap();
        assert_eq!(Project::open(&dir).unwrap().settings, settings);
    }

    #[test]
    fn names_are_checked() {
        assert!(is_valid_name("cellar 2"));
        assert!(is_valid_name("my-game_2"));
        assert!(!is_valid_name("  "));
        assert!(!is_valid_name("../escape"));
        assert!(!is_valid_name("a:b"));
    }
}
