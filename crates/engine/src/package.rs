//! Project packages: a whole game project in one file, to hand to someone
//! who opens it in their editor. Not a build of the game, just the project
//! folder as it is.
//!
//! A package is a zip archive (`.gamepack`) holding every file of the
//! project folder, except hidden ones, plus `package.ron`, a small manifest.
//! Opening one unpacks it into a new folder next to it.

use serde::{Deserialize, Serialize};
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};

/// File extension of packages.
pub const EXTENSION: &str = "gamepack";
/// The manifest's name inside the archive.
const MANIFEST: &str = "package.ron";

/// `package.ron`: what made the package.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub engine: String,
    pub engine_version: String,
    /// The game's name (from `game.ron`).
    pub name: String,
}

#[derive(Debug, thiserror::Error)]
pub enum PackageError {
    #[error("{0}: {1}")]
    Io(PathBuf, std::io::Error),
    #[error("{0}: {1}")]
    Zip(PathBuf, zip::result::ZipError),
    #[error("{0} is not a project package (no {MANIFEST})")]
    NotAPackage(PathBuf),
    #[error("{0}: bad {MANIFEST}: {1}")]
    Manifest(PathBuf, String),
    #[error("{0}: {1} would unpack outside the project folder")]
    UnsafeEntry(PathBuf, String),
}

/// Whether `path` looks like a package file.
pub fn is_package(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(EXTENSION))
}

/// Where EXPORT PACKAGE writes: next to the project folder, named after it.
pub fn export_path(project_dir: &Path) -> PathBuf {
    let dir = std::path::absolute(project_dir).unwrap_or_else(|_| project_dir.to_owned());
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "project".into());
    dir.with_file_name(format!("{name}.{EXTENSION}"))
}

/// Writes every file of the project to a package at `out`. Returns how many
/// files went in.
pub fn export(project_dir: &Path, name: &str, out: &Path) -> Result<usize, PackageError> {
    // Write next to the target first, so a failed export doesn't leave half
    // a package behind (or destroy the previous one).
    let partial = out.with_extension(format!("{EXTENSION}.partial"));
    let count = write(project_dir, name, &[], &partial)?;
    std::fs::rename(&partial, out).map_err(|e| PackageError::Io(out.to_owned(), e))?;
    Ok(count)
}

/// Writes the package itself to `out`: the manifest, every file of the
/// project, and `extras` (entry name in the archive, file on disk). Returns
/// how many project files went in.
pub(crate) fn write(
    project_dir: &Path,
    name: &str,
    extras: &[(&str, &Path)],
    out: &Path,
) -> Result<usize, PackageError> {
    let mut files = Vec::new();
    collect(project_dir, project_dir, &mut files)?;
    files.sort();

    let io = |path: &Path, err| PackageError::Io(path.to_owned(), err);
    let zip_err = |err| PackageError::Zip(out.to_owned(), err);
    let file = std::fs::File::create(out).map_err(|e| io(out, e))?;
    let mut zip = zip::ZipWriter::new(std::io::BufWriter::new(file));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    let manifest = Manifest {
        engine: crate::NAME.into(),
        engine_version: crate::VERSION.into(),
        name: name.into(),
    };
    let text =
        ron::ser::to_string_pretty(&manifest, Default::default()).expect("the manifest serializes");
    zip.start_file(MANIFEST, options).map_err(zip_err)?;
    zip.write_all(text.as_bytes()).map_err(|e| io(out, e))?;

    let project_files = files.iter().map(|relative| {
        // Zip paths always use forward slashes.
        let entry = relative
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        (entry, project_dir.join(relative))
    });
    let extras = extras
        .iter()
        .map(|(entry, path)| (entry.to_string(), path.to_path_buf()));
    for (entry, path) in project_files.chain(extras) {
        let bytes = std::fs::read(&path).map_err(|e| io(&path, e))?;
        zip.start_file(entry, options).map_err(zip_err)?;
        zip.write_all(&bytes).map_err(|e| io(out, e))?;
    }
    zip.finish()
        .map_err(zip_err)?
        .flush()
        .map_err(|e| io(out, e))?;
    Ok(files.len())
}

/// The project's files, relative to `root`: everything but hidden entries
/// (`.git` and the like) and packages.
fn collect(root: &Path, dir: &Path, files: &mut Vec<PathBuf>) -> Result<(), PackageError> {
    let entries = std::fs::read_dir(dir).map_err(|e| PackageError::Io(dir.to_owned(), e))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        if path.is_dir() {
            collect(root, &path, files)?;
        } else if !is_package(&path) && !path.to_string_lossy().ends_with(".partial") {
            let relative = path.strip_prefix(root).expect("walked from root");
            files.push(relative.to_owned());
        }
    }
    Ok(())
}

/// Unpacks a package into a new folder next to it, named after the file
/// (`hall.gamepack` → `hall/`, or `hall 2/` if that's taken). Returns the
/// folder and the manifest.
pub fn unpack(file: &Path) -> Result<(PathBuf, Manifest), PackageError> {
    let target = free_folder(&file.with_extension(""));
    unpack_into(file, &target).map(|manifest| (target, manifest))
}

/// Where a package goes when unpacked into the folder `parent`: `parent`
/// itself if it is empty, otherwise a new folder in it named after the
/// file (`hall.gamepack` → `hall/`, or `hall 2/` if that's taken).
pub fn unpack_target(file: &Path, parent: &Path) -> PathBuf {
    let empty = parent
        .read_dir()
        .is_ok_and(|mut entries| entries.next().is_none());
    if empty {
        return parent.to_owned();
    }
    let stem = file.file_stem().unwrap_or_default();
    free_folder(&parent.join(stem))
}

/// Unpacks a package into `target` (created if missing).
pub fn unpack_into(file: &Path, target: &Path) -> Result<Manifest, PackageError> {
    let reader = std::fs::File::open(file).map_err(|e| PackageError::Io(file.to_owned(), e))?;
    unpack_from(std::io::BufReader::new(reader), file, target)
}

/// The manifest of the package read from `reader`; `file` names it in
/// errors.
pub(crate) fn manifest_of<R: Read + Seek>(
    reader: R,
    file: &Path,
) -> Result<Manifest, PackageError> {
    let mut zip =
        zip::ZipArchive::new(reader).map_err(|e| PackageError::Zip(file.to_owned(), e))?;
    read_manifest(&mut zip, file)
}

fn read_manifest<R: Read + Seek>(
    zip: &mut zip::ZipArchive<R>,
    file: &Path,
) -> Result<Manifest, PackageError> {
    let mut entry = zip
        .by_name(MANIFEST)
        .map_err(|_| PackageError::NotAPackage(file.to_owned()))?;
    let mut text = String::new();
    entry
        .read_to_string(&mut text)
        .map_err(|e| PackageError::Manifest(file.to_owned(), e.to_string()))?;
    ron::from_str::<Manifest>(&text)
        .map_err(|e| PackageError::Manifest(file.to_owned(), e.to_string()))
}

/// Unpacks the package read from `reader` into `target` (created if
/// missing); `file` names it in errors.
pub(crate) fn unpack_from<R: Read + Seek>(
    reader: R,
    file: &Path,
    target: &Path,
) -> Result<Manifest, PackageError> {
    let io = |path: &Path, err| PackageError::Io(path.to_owned(), err);
    let zip_err = |err| PackageError::Zip(file.to_owned(), err);
    let mut zip = zip::ZipArchive::new(reader).map_err(zip_err)?;

    let manifest = read_manifest(&mut zip, file)?;
    if manifest.engine_version != crate::VERSION {
        log::warn!(
            "{} was made with {} {} (this is {})",
            file.display(),
            manifest.engine,
            manifest.engine_version,
            crate::VERSION
        );
    }

    std::fs::create_dir_all(target).map_err(|e| io(target, e))?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(zip_err)?;
        let name = entry.name().map_err(zip_err)?.into_owned();
        if name == MANIFEST || entry.is_dir() {
            continue;
        }
        // Refuse `../` and absolute paths: a package must not write outside
        // its folder.
        let Some(relative) = entry.enclosed_name() else {
            return Err(PackageError::UnsafeEntry(file.to_owned(), name));
        };
        let path = target.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| io(parent, e))?;
        }
        let mut out = std::fs::File::create(&path).map_err(|e| io(&path, e))?;
        std::io::copy(&mut entry, &mut out).map_err(|e| io(&path, e))?;
    }
    Ok(manifest)
}

/// `wanted`, or `wanted 2`, `wanted 3`… whichever doesn't exist yet.
fn free_folder(wanted: &Path) -> PathBuf {
    if !wanted.exists() {
        return wanted.to_owned();
    }
    let name = wanted
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    (2..)
        .map(|n| wanted.with_file_name(format!("{name} {n}")))
        .find(|p| !p.exists())
        .expect("some number is free")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("engine-package-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_sandbox_round_trips() {
        let sandbox = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sandbox");
        let dir = temp_dir("round-trip");
        let file = dir.join("sandbox.gamepack");
        let count = export(&sandbox, "Sandbox", &file).unwrap();

        let (unpacked, manifest) = unpack(&file).unwrap();
        assert_eq!(unpacked, dir.join("sandbox"));
        assert_eq!(manifest.name, "Sandbox");
        let mut files = Vec::new();
        collect(&unpacked, &unpacked, &mut files).unwrap();
        assert_eq!(files.len(), count);
        for relative in &files {
            assert_eq!(
                std::fs::read(sandbox.join(relative)).unwrap(),
                std::fs::read(unpacked.join(relative)).unwrap(),
                "{}",
                relative.display()
            );
        }
        let project = crate::project::Project::open(&unpacked).unwrap();
        project.load_start_scene().unwrap();

        // Opening it again doesn't overwrite the first copy.
        assert_eq!(unpack(&file).unwrap().0, dir.join("sandbox 2"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn packages_unpack_into_an_empty_folder_or_a_new_one_in_it() {
        let dir = temp_dir("target");
        let file = Path::new("elsewhere/hall.gamepack");
        let empty = dir.join("empty");
        std::fs::create_dir(&empty).unwrap();
        assert_eq!(unpack_target(file, &empty), empty);
        std::fs::write(empty.join("note.txt"), "x").unwrap();
        assert_eq!(unpack_target(file, &empty), empty.join("hall"));
        std::fs::create_dir(empty.join("hall")).unwrap();
        assert_eq!(unpack_target(file, &empty), empty.join("hall 2"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_plain_zip_is_not_a_package() {
        let dir = temp_dir("plain-zip");
        let file = dir.join("other.gamepack");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&file).unwrap());
        zip.start_file("game.ron", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.finish().unwrap();
        assert!(matches!(
            unpack_into(&file, &dir.join("out")),
            Err(PackageError::NotAPackage(_))
        ));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
