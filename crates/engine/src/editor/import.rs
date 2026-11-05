//! Bringing model files into the game project. A file dropped on the editor
//! is copied into the project's `models/` folder, so scenes only reference
//! files inside the project.

use std::path::{Component, Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("{0}: only .glb and .gltf models can be imported")]
    NotAModel(PathBuf),
    #[error("could not copy {from} to {to}: {source}")]
    Copy {
        from: PathBuf,
        to: PathBuf,
        source: std::io::Error,
    },
    #[error("{path}: {source}")]
    Gltf { path: PathBuf, source: gltf::Error },
    #[error("{path} references \"{uri}\", which isn't in its folder")]
    OutsideFolder { path: PathBuf, uri: String },
    #[error("{0} already exists with different contents")]
    Conflict(PathBuf),
}

pub fn is_model(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("glb") || e.eq_ignore_ascii_case("gltf"))
}

/// Copies a model into `<project>/models/` and returns its path relative to
/// the project, with `/` separators as scenes store it.
///
/// A file already inside the project is used where it is. If `models/`
/// has an identical file of the same name, that one is reused; a different
/// one is never overwritten, the copy gets a numbered name instead. A
/// `.gltf` also brings the buffer and image files it references.
pub fn copy_into_project(file: &Path, project: &Path) -> Result<String, ImportError> {
    if !is_model(file) {
        return Err(ImportError::NotAModel(file.to_owned()));
    }
    if let Some(relative) = inside(file, project) {
        return Ok(relative);
    }
    let models = project.join("models");

    // Check everything before copying anything, so a failure leaves no half import.
    let source_dir = file.parent().unwrap_or(Path::new(""));
    let mut copies = Vec::new();
    for uri in dependencies(file)? {
        let (from, to) = (source_dir.join(&uri), models.join(&uri));
        if !to.exists() {
            copies.push((from, to));
        } else if !same_contents(&from, &to) {
            return Err(ImportError::Conflict(to));
        }
    }
    let target = free_name(file, &models);
    if !target.exists() {
        copies.push((file.to_owned(), target.clone()));
    }

    for (from, to) in copies {
        let copied = to
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::copy(&from, &to));
        copied.map_err(|source| ImportError::Copy { from, to, source })?;
    }
    let name = target.file_name().unwrap_or_default().to_string_lossy();
    Ok(format!("models/{name}"))
}

/// `file`'s path relative to `project`, if it's inside it.
fn inside(file: &Path, project: &Path) -> Option<String> {
    let (file, project) = (file.canonicalize().ok()?, project.canonicalize().ok()?);
    let relative = file.strip_prefix(project).ok()?;
    let parts: Vec<_> = relative
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect();
    Some(parts.join("/"))
}

/// `models/<name>`, or `models/<stem>-2.<ext>`, `-3`... if a different file
/// already has that name. An identical file's name is returned as is.
fn free_name(file: &Path, models: &Path) -> PathBuf {
    let name = file.file_name().unwrap_or_default();
    let stem = file.file_stem().unwrap_or_default().to_string_lossy();
    let ext = file.extension().unwrap_or_default().to_string_lossy();
    std::iter::once(models.join(name))
        .chain((2..).map(|n| models.join(format!("{stem}-{n}.{ext}"))))
        .find(|candidate| !candidate.exists() || same_contents(file, candidate))
        .expect("some number is free")
}

/// Files a `.gltf` references by relative path (buffers, images).
fn dependencies(file: &Path) -> Result<Vec<String>, ImportError> {
    let is_gltf = file
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("gltf"));
    if !is_gltf {
        return Ok(Vec::new()); // .glb is self-contained
    }
    let gltf = gltf::Gltf::open(file).map_err(|source| ImportError::Gltf {
        path: file.to_owned(),
        source,
    })?;
    let buffers = gltf.buffers().filter_map(|b| match b.source() {
        gltf::buffer::Source::Uri(uri) => Some(uri.to_owned()),
        gltf::buffer::Source::Bin => None,
    });
    let images = gltf.images().filter_map(|i| match i.source() {
        gltf::image::Source::Uri { uri, .. } => Some(uri.to_owned()),
        gltf::image::Source::View { .. } => None,
    });
    let mut uris = Vec::new();
    for uri in buffers.chain(images).filter(|u| !u.starts_with("data:")) {
        if !Path::new(&uri)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
        {
            return Err(ImportError::OutsideFolder {
                path: file.to_owned(),
                uri,
            });
        }
        uris.push(uri);
    }
    Ok(uris)
}

fn same_contents(a: &Path, b: &Path) -> bool {
    matches!((std::fs::read(a), std::fs::read(b)), (Ok(a), Ok(b)) if a == b)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh, empty folder under the system temp folder.
    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("engine-import-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn copies_and_never_overwrites() {
        let root = temp_dir("glb");
        let (project, outside) = (root.join("project"), root.join("downloads"));
        std::fs::create_dir_all(&outside).unwrap();
        let file = outside.join("door.glb");
        std::fs::write(&file, b"one").unwrap();

        assert_eq!(
            copy_into_project(&file, &project).unwrap(),
            "models/door.glb"
        );
        // Same file again: reused.
        assert_eq!(
            copy_into_project(&file, &project).unwrap(),
            "models/door.glb"
        );
        // Different file, same name: numbered.
        std::fs::write(&file, b"two").unwrap();
        assert_eq!(
            copy_into_project(&file, &project).unwrap(),
            "models/door-2.glb"
        );
        assert_eq!(
            std::fs::read(project.join("models/door.glb")).unwrap(),
            b"one"
        );
        // A file already in the project stays where it is.
        let inner = project.join("models/door.glb");
        assert_eq!(
            copy_into_project(&inner, &project).unwrap(),
            "models/door.glb"
        );
        assert!(matches!(
            copy_into_project(&outside.join("notes.txt"), &project),
            Err(ImportError::NotAModel(_))
        ));
    }

    #[test]
    fn gltf_brings_its_buffers() {
        let root = temp_dir("gltf");
        let (project, outside) = (root.join("project"), root.join("export"));
        std::fs::create_dir_all(&outside).unwrap();
        let file = outside.join("lamp.gltf");
        std::fs::write(
            &file,
            r#"{"asset":{"version":"2.0"},"buffers":[{"byteLength":4,"uri":"lamp.bin"}]}"#,
        )
        .unwrap();
        std::fs::write(outside.join("lamp.bin"), [0u8; 4]).unwrap();

        assert_eq!(
            copy_into_project(&file, &project).unwrap(),
            "models/lamp.gltf"
        );
        assert!(project.join("models/lamp.bin").exists());

        // A different buffer of the same name would break the existing model.
        std::fs::write(outside.join("lamp.bin"), [1u8; 4]).unwrap();
        assert!(matches!(
            copy_into_project(&file, &project),
            Err(ImportError::Conflict(_))
        ));
    }
}
