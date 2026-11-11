//! Standalone games: the game as one executable, for players.
//!
//! EXPORT GAME copies the engine's own executable and adds the project to
//! its end as a package (see `package`), with Steam Audio's library inside
//! it on Windows. Started, such an executable finds the package in itself,
//! unpacks it once into the user's cache folder and plays it as a player
//! gets it (`--player`).
//!
//! After the executable's own bytes come the package, then a 24-byte
//! trailer: a hash of the package, its length (both little-endian `u64`)
//! and `MAGIC`.

use crate::package::{self, PackageError};
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// Ends an executable that holds a game.
const MAGIC: &[u8; 8] = b"GAME.EXE";
const TRAILER: u64 = 24;

/// Steam Audio's library, which the engine loads when it starts.
const LIBRARY: &str = if cfg!(windows) {
    "phonon.dll"
} else if cfg!(target_os = "macos") {
    "libphonon.dylib"
} else {
    "libphonon.so"
};
/// Where the library sits in the package (a hidden folder, so it never
/// counts as a project file).
const LIBRARY_FOLDER: &str = ".engine";
/// In the unpacked folder: the hash of the package it came from.
const STAMP: &str = ".build";

#[derive(Debug, thiserror::Error)]
pub enum StandaloneError {
    #[error(transparent)]
    Package(#[from] PackageError),
    #[error("{0}: {1}")]
    Io(PathBuf, std::io::Error),
    #[error("can't find {LIBRARY} (Steam Audio) next to the engine or on the library path")]
    NoLibrary,
    #[error("this system has no folder to unpack the game into")]
    NoCacheFolder,
    #[error("can't load {0}")]
    Library(PathBuf),
}

fn io(path: &Path) -> impl Fn(std::io::Error) -> StandaloneError + '_ {
    |err| StandaloneError::Io(path.to_owned(), err)
}

/// Where EXPORT GAME writes: next to the project folder, named after it.
pub fn export_path(project_dir: &Path) -> PathBuf {
    package::export_path(project_dir).with_extension(std::env::consts::EXE_EXTENSION)
}

/// Writes the project as a game executable at `out`, built on the engine
/// that is running. Returns how many project files went in.
pub fn export(project_dir: &Path, name: &str, out: &Path) -> Result<usize, StandaloneError> {
    let engine = std::env::current_exe().map_err(io(Path::new("the engine")))?;
    let library = find_library().ok_or(StandaloneError::NoLibrary)?;
    write(&engine, &library, project_dir, name, out)
}

/// `export` with the engine executable and the library given.
fn write(
    engine: &Path,
    library: &Path,
    project_dir: &Path,
    name: &str,
    out: &Path,
) -> Result<usize, StandaloneError> {
    // Both written next to the target first, so a failed export leaves the
    // previous game as it was.
    let pack = out.with_extension("pack.partial");
    let partial = out.with_extension("exe.partial");

    // Windows finds the library through the executable (it is unpacked and
    // loaded before anything uses it); elsewhere the system's loader wants
    // it on disk before the program starts, so it goes next to the game.
    let entry = format!("{LIBRARY_FOLDER}/{LIBRARY}");
    let extras: &[(&str, &Path)] = if cfg!(windows) {
        &[(&entry, library)]
    } else {
        &[]
    };
    let count = package::write(project_dir, name, extras, &pack)?;

    let result = (|| {
        let mut game = File::create(&partial).map_err(io(&partial))?;
        let mut source = File::open(engine).map_err(io(engine))?;
        // An engine that is itself a game: only the engine's part.
        let own = match find_payload(&mut source).map_err(io(engine))? {
            Some(payload) => payload.start,
            None => source.metadata().map_err(io(engine))?.len(),
        };
        source.rewind().map_err(io(engine))?;
        std::io::copy(&mut source.take(own), &mut game).map_err(io(&partial))?;

        let bytes = std::fs::read(&pack).map_err(io(&pack))?;
        game.write_all(&bytes).map_err(io(&partial))?;
        game.write_all(&crate::save::fnv(&bytes).to_le_bytes())
            .map_err(io(&partial))?;
        game.write_all(&(bytes.len() as u64).to_le_bytes())
            .map_err(io(&partial))?;
        game.write_all(MAGIC).map_err(io(&partial))?;
        game.flush().map_err(io(&partial))?;
        drop(game);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::Permissions::from_mode(0o755);
            std::fs::set_permissions(&partial, mode).map_err(io(&partial))?;
        }
        std::fs::rename(&partial, out).map_err(io(out))?;
        if !cfg!(windows) {
            let beside = out.with_file_name(LIBRARY);
            std::fs::copy(library, &beside).map_err(io(&beside))?;
        }
        Ok(count)
    })();
    let _ = std::fs::remove_file(&pack);
    if result.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    result
}

/// The library the running engine uses: next to it, or on the system's
/// library path (where `cargo run` puts the one it built against).
fn find_library() -> Option<PathBuf> {
    let beside = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    let variable = if cfg!(windows) {
        "PATH"
    } else if cfg!(target_os = "macos") {
        "DYLD_LIBRARY_PATH"
    } else {
        "LD_LIBRARY_PATH"
    };
    let on_path = std::env::var_os(variable)
        .map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .unwrap_or_default();
    beside
        .into_iter()
        .chain(on_path)
        .map(|dir| dir.join(LIBRARY))
        .find(|file| file.is_file())
}

/// The package at the end of an executable.
struct Payload {
    /// Where it starts in the file.
    start: u64,
    len: u64,
    hash: u64,
}

fn find_payload(file: &mut File) -> std::io::Result<Option<Payload>> {
    let len = file.metadata()?.len();
    if len < TRAILER {
        return Ok(None);
    }
    let mut trailer = [0u8; TRAILER as usize];
    file.seek(SeekFrom::Start(len - TRAILER))?;
    file.read_exact(&mut trailer)?;
    if &trailer[16..] != MAGIC {
        return Ok(None);
    }
    let number = |at: usize| u64::from_le_bytes(trailer[at..at + 8].try_into().expect("8 bytes"));
    let (hash, pack) = (number(0), number(8));
    Ok((pack <= len - TRAILER).then(|| Payload {
        start: len - TRAILER - pack,
        len: pack,
        hash,
    }))
}

/// A part of a file, read as if it were the whole file.
struct Window {
    file: File,
    start: u64,
    len: u64,
    at: u64,
}

impl Read for Window {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let left = self.len.saturating_sub(self.at);
        let wanted = buf.len().min(left as usize);
        self.file.seek(SeekFrom::Start(self.start + self.at))?;
        let read = self.file.read(&mut buf[..wanted])?;
        self.at += read as u64;
        Ok(read)
    }
}

impl Seek for Window {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        let at = match to {
            SeekFrom::Start(at) => Some(at),
            SeekFrom::End(by) => self.len.checked_add_signed(by),
            SeekFrom::Current(by) => self.at.checked_add_signed(by),
        };
        self.at = at.ok_or_else(|| std::io::Error::other("seek before the start"))?;
        Ok(self.at)
    }
}

/// The game built into the running executable, unpacked and ready to play:
/// its project folder. `None` for the engine on its own.
pub fn embedded_game() -> Result<Option<PathBuf>, StandaloneError> {
    let exe = std::env::current_exe().map_err(io(Path::new("the game")))?;
    let games = cache_dir().map(|dir| dir.join("games"));
    let Some(dir) = unpack_game(&exe, games.as_deref())? else {
        return Ok(None);
    };
    let library = dir.join(LIBRARY_FOLDER).join(LIBRARY);
    if library.is_file() {
        load_library(&library)?;
    }
    Ok(Some(dir))
}

/// Unpacks the game in the executable `exe`, if it holds one, into a folder
/// of its own in `games`, unless that was done already. Returns the folder.
///
/// The folder is named after the game, not the build, so a newer build of
/// the game replaces the older one's files and keeps its saved game (saves
/// go by the project folder's path).
fn unpack_game(exe: &Path, games: Option<&Path>) -> Result<Option<PathBuf>, StandaloneError> {
    let mut file = File::open(exe).map_err(io(exe))?;
    let Some(payload) = find_payload(&mut file).map_err(io(exe))? else {
        return Ok(None);
    };
    let games = games.ok_or(StandaloneError::NoCacheFolder)?;
    let window = |file: File| {
        BufReader::new(Window {
            file,
            start: payload.start,
            len: payload.len,
            at: 0,
        })
    };
    let manifest = package::manifest_of(window(file.try_clone().map_err(io(exe))?), exe)?;
    let dir = games.join(folder_name(&manifest.name));

    let stamp = dir.join(STAMP);
    let build = format!("{:016x}", payload.hash);
    if std::fs::read_to_string(&stamp).is_ok_and(|have| have == build) {
        return Ok(Some(dir));
    }
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(io(&dir))?;
    }
    package::unpack_from(window(file), exe, &dir)?;
    // Last, so a half-unpacked folder is unpacked again.
    std::fs::write(&stamp, build).map_err(io(&stamp))?;
    log::info!("unpacked {} into {}", manifest.name, dir.display());
    Ok(Some(dir))
}

/// A folder name for the game called `name`: letters, digits, spaces, `-`
/// and `_` are kept.
fn folder_name(name: &str) -> String {
    let kept: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let kept = kept.trim();
    if kept.is_empty() {
        "game".into()
    } else {
        kept.into()
    }
}

/// The engine's folder in the user's cache folder; `None` if the system
/// has none.
fn cache_dir() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("XDG_CACHE_HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    Some(base.join(crate::NAME))
}

/// Loads the library from the unpacked game, so the engine's first call
/// into Steam Audio finds it (the app links it delay-loaded on Windows).
#[cfg(windows)]
fn load_library(path: &Path) -> Result<(), StandaloneError> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::LibraryLoader::LoadLibraryW;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: `wide` is a NUL-terminated path that outlives the call.
    let handle = unsafe { LoadLibraryW(wide.as_ptr()) };
    if handle as usize == 0 {
        return Err(StandaloneError::Library(path.to_owned()));
    }
    Ok(())
}

#[cfg(not(windows))]
fn load_library(_path: &Path) -> Result<(), StandaloneError> {
    Ok(())
}

/// Tells the player why the game didn't start: a game has no console.
pub fn show_error(text: &str) {
    rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Error)
        .set_title("The game couldn't start")
        .set_description(text)
        .show();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("engine-standalone-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_sandbox_plays_from_an_executable() {
        let sandbox = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sandbox");
        let dir = temp_dir("round-trip");
        let engine = dir.join("engine.bin");
        let library = dir.join("library.bin");
        std::fs::write(&engine, b"the engine itself").unwrap();
        std::fs::write(&library, b"steam audio").unwrap();
        let game = dir.join("game.exe");
        let count = write(&engine, &library, &sandbox, "Sand/box", &game).unwrap();
        assert!(count > 0);
        assert!(
            std::fs::read(&game)
                .unwrap()
                .starts_with(b"the engine itself")
        );
        assert!(!dir.join("game.pack.partial").exists());
        assert!(!dir.join("game.exe.partial").exists());

        let games = dir.join("games");
        let unpacked = unpack_game(&game, Some(&games)).unwrap().unwrap();
        assert_eq!(unpacked, games.join("Sand_box"));
        let project = crate::project::Project::open(&unpacked).unwrap();
        project.load_start_scene().unwrap();
        if cfg!(windows) {
            let inside = unpacked.join(LIBRARY_FOLDER).join(LIBRARY);
            assert_eq!(std::fs::read(inside).unwrap(), b"steam audio");
        } else {
            assert_eq!(std::fs::read(dir.join(LIBRARY)).unwrap(), b"steam audio");
        }

        // The same build is not unpacked twice…
        let note = unpacked.join("note.txt");
        std::fs::write(&note, "x").unwrap();
        assert_eq!(unpack_game(&game, Some(&games)).unwrap(), Some(unpacked));
        assert!(note.exists());

        // …a game exported from a game holds the engine once, and a build
        // with another name is unpacked into its own folder.
        let second = dir.join("second.exe");
        write(&game, &library, &sandbox, "Second", &second).unwrap();
        assert!(
            std::fs::metadata(&second).unwrap().len() < 2 * std::fs::metadata(&game).unwrap().len()
        );
        let other = unpack_game(&second, Some(&games)).unwrap().unwrap();
        assert_eq!(other, games.join("Second"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_new_build_replaces_the_old_files() {
        let dir = temp_dir("rebuild");
        let (engine, library) = (dir.join("engine.bin"), dir.join("library.bin"));
        std::fs::write(&engine, b"engine").unwrap();
        std::fs::write(&library, b"library").unwrap();
        let project = dir.join("project");
        std::fs::create_dir(&project).unwrap();
        std::fs::write(project.join("game.ron"), "one").unwrap();
        std::fs::write(project.join("old.txt"), "x").unwrap();
        let game = dir.join("game.exe");
        let games = dir.join("games");
        write(&engine, &library, &project, "Game", &game).unwrap();
        let unpacked = unpack_game(&game, Some(&games)).unwrap().unwrap();

        std::fs::write(project.join("game.ron"), "two").unwrap();
        std::fs::remove_file(project.join("old.txt")).unwrap();
        write(&engine, &library, &project, "Game", &game).unwrap();
        assert_eq!(
            unpack_game(&game, Some(&games)).unwrap(),
            Some(unpacked.clone())
        );
        assert_eq!(
            std::fs::read_to_string(unpacked.join("game.ron")).unwrap(),
            "two"
        );
        assert!(!unpacked.join("old.txt").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_engine_alone_holds_no_game() {
        let dir = temp_dir("plain");
        let engine = dir.join("engine.bin");
        std::fs::write(&engine, vec![7u8; 100]).unwrap();
        assert_eq!(unpack_game(&engine, Some(&dir)).unwrap(), None);
        std::fs::write(&engine, b"tiny").unwrap();
        assert_eq!(unpack_game(&engine, Some(&dir)).unwrap(), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn folder_names_are_safe() {
        assert_eq!(folder_name("The Quiet Hall"), "The Quiet Hall");
        assert_eq!(folder_name("../.."), "_____");
        assert_eq!(folder_name("  "), "game");
    }
}
