//! The project's files the editor offers to open.

use super::import;
use crate::modeler::glb;
use std::path::{Path, PathBuf};

/// Scene files in the project's `scenes/` folder, relative to the project.
pub(super) fn scene_files(project_dir: &std::path::Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(project_dir.join("scenes")) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "ron"))
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

/// Models in the project's `models/` folder that the modeler can open.
pub fn model_files(project_dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(project_dir.join("models")) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("glb")))
        .filter(|p| glb::is_editable(p))
        .filter_map(|p| p.strip_prefix(project_dir).ok().map(PathBuf::from))
        .collect();
    files.sort();
    files
}
