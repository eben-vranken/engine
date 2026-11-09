//! The projects opened in the editor before, most recent first, for the
//! launcher. Kept in the user's config folder (`%APPDATA%\engine` on
//! Windows, `~/.config/engine` on Linux), never in a project.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// The list keeps this many.
const KEEP: usize = 30;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecentProject {
    pub dir: PathBuf,
    /// The game's name when it was last opened.
    pub name: String,
    /// When it was last opened, seconds since 1970.
    pub opened: u64,
}

/// The engine's folder in the user's config folder; `None` if the system
/// has none.
pub fn config_dir() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join(crate::NAME))
}

/// Where the list lives; `None` if the system has no config folder.
fn file() -> Option<PathBuf> {
    Some(config_dir()?.join("recent.ron"))
}

/// The list, most recent first; empty if there is none (or it can't be
/// read).
pub fn load() -> Vec<RecentProject> {
    let Some(text) = file().and_then(|f| std::fs::read_to_string(f).ok()) else {
        return Vec::new();
    };
    let mut list: Vec<RecentProject> = ron::from_str(&text).unwrap_or_else(|err| {
        log::warn!("recent projects: {err}");
        Vec::new()
    });
    list.sort_by_key(|p| std::cmp::Reverse(p.opened));
    list
}

fn save(list: &[RecentProject]) {
    let Some(path) = file() else {
        return;
    };
    let written = ron::ser::to_string_pretty(list, Default::default())
        .map_err(|e| e.to_string())
        .and_then(|text| {
            path.parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| std::fs::write(&path, text))
                .map_err(|e| e.to_string())
        });
    if let Err(err) = written {
        log::warn!(
            "could not save the recent projects to {}: {err}",
            path.display()
        );
    }
}

/// Notes that the project in `dir` called `name` was just opened.
pub fn record(dir: &Path, name: &str) {
    save(&opened(load(), &absolute(dir), name, now()));
}

/// Takes `dir` off the list.
pub fn forget(dir: &Path) {
    let mut list = load();
    list.retain(|p| p.dir != dir);
    save(&list);
}

/// `list` with `dir` moved to the front (or added), opened at `when`.
fn opened(mut list: Vec<RecentProject>, dir: &Path, name: &str, when: u64) -> Vec<RecentProject> {
    list.retain(|p| p.dir != dir);
    list.insert(
        0,
        RecentProject {
            dir: dir.to_owned(),
            name: name.to_owned(),
            opened: when,
        },
    );
    list.truncate(KEEP);
    list
}

/// `dir` as a full path, so the same project is one entry however it was
/// named (Windows' `\\?\` prefix dropped, it only confuses people).
fn absolute(dir: &Path) -> PathBuf {
    let Ok(full) = dir.canonicalize() else {
        return dir.to_owned();
    };
    let text = full.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(plain) => PathBuf::from(plain),
        None => full,
    }
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// "TODAY", "YESTERDAY", "3 DAYS AGO", … for `opened`, seen at `now`.
pub fn ago(opened: u64, now: u64) -> String {
    const DAY: u64 = 24 * 60 * 60;
    let seconds = now.saturating_sub(opened);
    match seconds / DAY {
        _ if seconds < 60 * 60 => "JUST NOW".into(),
        0 => format!("{} HOURS AGO", (seconds / 3600).max(1)),
        1 => "YESTERDAY".into(),
        days if days < 60 => format!("{days} DAYS AGO"),
        days => format!("{} MONTHS AGO", days / 30),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reopening_moves_a_project_to_the_front() {
        let list = opened(Vec::new(), Path::new("/a"), "a", 10);
        let list = opened(list, Path::new("/b"), "b", 20);
        let list = opened(list, Path::new("/a"), "a renamed", 30);
        let names: Vec<&str> = list.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["a renamed", "b"], "one entry each, newest first");

        let mut list = Vec::new();
        for i in 0..KEEP + 5 {
            list = opened(list, &PathBuf::from(format!("/{i}")), "x", i as u64);
        }
        assert_eq!(list.len(), KEEP);
    }

    #[test]
    fn ages_read_like_people_say_them() {
        let day = 24 * 60 * 60;
        assert_eq!(ago(100, 200), "JUST NOW");
        assert_eq!(ago(0, 5 * 3600), "5 HOURS AGO");
        assert_eq!(ago(0, day + 10), "YESTERDAY");
        assert_eq!(ago(0, 3 * day), "3 DAYS AGO");
        assert_eq!(ago(0, 90 * day), "3 MONTHS AGO");
    }
}
