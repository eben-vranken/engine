//! The project's files the editor offers to open.

use super::import;
use std::path::{Path, PathBuf};

/// Scene files in the project's `scenes/` folder, relative to the project.
pub(super) fn scene_files(project_dir: &Path) -> Vec<PathBuf> {
    files_in(project_dir, "scenes", "ron")
}

/// Graph files in the project's `graphs/` folder, relative to the project.
pub(super) fn graph_files(project_dir: &Path) -> Vec<PathBuf> {
    files_in(project_dir, "graphs", "ron")
}

/// Every model the scene can place: `.glb` and `.gltf` files anywhere in
/// the project's `models/` folder, relative to the project, sorted.
pub(super) fn all_models(project_dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut folders = vec![project_dir.join("models")];
    while let Some(folder) = folders.pop() {
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        for path in entries.flatten().map(|e| e.path()) {
            if path.is_dir() {
                folders.push(path);
            } else if import::is_model(&path)
                && let Ok(relative) = path.strip_prefix(project_dir)
            {
                found.push(relative.to_path_buf());
            }
        }
    }
    found.sort();
    found
}

/// The `.<extension>` files directly in `<project>/<folder>`, relative to
/// the project, sorted.
fn files_in(project_dir: &Path, folder: &str, extension: &str) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(project_dir.join(folder)) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .is_some_and(|e| e.eq_ignore_ascii_case(extension))
        })
        .filter_map(|p| p.strip_prefix(project_dir).ok().map(PathBuf::from))
        .collect();
    files.sort();
    files
}

/// Every `.wav` in the project, as paths relative to it with `/`
/// separators, sorted. Hidden folders are skipped.
pub(super) fn sound_files(project_dir: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut folders = vec![project_dir.to_path_buf()];
    while let Some(folder) = folders.pop() {
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let hidden = entry.file_name().to_string_lossy().starts_with('.');
            if path.is_dir() {
                if !hidden {
                    folders.push(path);
                }
            } else if import::is_sound(&path)
                && let Ok(relative) = path.strip_prefix(project_dir)
            {
                let parts: Vec<_> = relative
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy())
                    .collect();
                found.push(parts.join("/"));
            }
        }
    }
    found.sort();
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sandbox_models_are_listed() {
        let sandbox = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sandbox");
        let models = all_models(&sandbox);
        assert!(
            models.contains(&PathBuf::from("models/arch.gltf")),
            "{models:?}"
        );
        assert!(models.iter().all(|p| import::is_model(p)));
    }
}
