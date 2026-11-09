//! What reference pickers offer from the whole project: flags, graphs,
//! scenes, sounds, and the objects, zones and characters of every scene
//! (graphs span scenes). Looked at again every few seconds, so new files
//! and saved scenes show up without a restart.

use super::files::{graph_files, scene_files, sound_files};
use super::inspector::Names;
use crate::graph::flags::FlagDefs;
use crate::graph::key;
use crate::scene::{Component, Scene};
use std::path::Path;
use std::time::{Duration, Instant};

const RESCAN: Duration = Duration::from_secs(3);

#[derive(Default)]
pub(super) struct ProjectIndex {
    scanned: Option<Instant>,
    pub flags: FlagDefs,
    pub names: Names,
    /// `flags` was edited and isn't written yet (a rescan keeps it).
    pub flags_unsaved: bool,
}

impl ProjectIndex {
    /// Looks at the project again if it's been a while (or `force`).
    pub fn refresh(&mut self, dir: &Path, force: bool) {
        if !force && self.scanned.is_some_and(|t| t.elapsed() < RESCAN) {
            return;
        }
        self.scanned = Some(Instant::now());
        if !self.flags_unsaved {
            self.flags = FlagDefs::load(dir);
        }
        let scenes = scene_files(dir);
        let mut names = Names {
            flags: self.flags.names(),
            graphs: graph_files(dir).iter().map(|p| key(p)).collect(),
            scenes: scenes.iter().map(|p| key(p)).collect(),
            sounds: sound_files(dir),
            ..Default::default()
        };
        for path in &scenes {
            let Ok(scene) = Scene::load(&dir.join(path)) else {
                continue;
            };
            names
                .zones
                .extend(scene.zones.iter().map(|z| z.name.clone()));
            for object in &scene.objects {
                names.objects.push(object.name.clone());
                for component in &object.components {
                    if let Component::Character(c) = component
                        && !c.id.is_empty()
                    {
                        names.characters.push(c.id.clone());
                    }
                }
            }
        }
        for list in [
            &mut names.objects,
            &mut names.zones,
            &mut names.characters,
            &mut names.flags,
        ] {
            list.sort();
            list.dedup();
        }
        self.names = names;
    }

    /// The pickers' lists for editing `scene`: its own objects and zones,
    /// the rest from the project.
    pub fn for_scene(&self, scene: &Scene) -> Names {
        let mut objects: Vec<String> = scene.objects.iter().map(|o| o.name.clone()).collect();
        objects.sort();
        Names {
            objects,
            zones: scene.zones.iter().map(|z| z.name.clone()).collect(),
            ..self.names.clone()
        }
    }
}
