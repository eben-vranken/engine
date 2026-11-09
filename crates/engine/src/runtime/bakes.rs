//! Reverb and light bakes: loading them, running them in the background,
//! and whether they still match the scene.

use super::Running;
use crate::audio::bake;
use crate::lightmap;
use std::path::PathBuf;

impl Running {
    /// The current scene's file, in the project.
    pub(super) fn scene_file(&self) -> PathBuf {
        let relative = match (&self.play_scene, &self.editor) {
            (Some(path), _) => path,
            (None, Some(editor)) => &editor.scene_path,
            (None, None) => &self.scene_path,
        };
        self.game_dir.join(relative)
    }

    /// Loads the scene's reverb bake (if it has one) into the audio, and
    /// its probes into the editor.
    pub(super) fn load_bake(&mut self) {
        let file = bake::path_for(&self.scene_file());
        let Some(audio) = &mut self.audio else {
            return;
        };
        let (signature, probes) = match audio.load_bake(&file) {
            Ok(Some((signature, probes))) => (Some(signature), probes),
            Ok(None) => (None, Vec::new()),
            Err(err) => {
                log::error!("{err}");
                if let Some(editor) = &mut self.editor {
                    editor.show_error(err.to_string());
                }
                (None, Vec::new())
            }
        };
        self.baked_signature = signature;
        if let Some(editor) = &mut self.editor {
            editor.probes = probes;
        }
        self.check_bake();
    }

    /// Editor: starts baking the scene's reverb.
    pub(super) fn start_bake(&mut self) {
        if self.bake_job.is_some() {
            return;
        }
        let file = bake::path_for(&self.scene_file());
        let result = match &self.audio {
            Some(audio) => audio
                .start_bake(&self.scene, &mut self.assets, file)
                .map_err(|e| e.to_string()),
            None => Err("NO AUDIO DEVICE: CAN'T BAKE".into()),
        };
        match result {
            Ok(job) => self.bake_job = Some(job),
            Err(err) => {
                log::error!("{err}");
                if let Some(editor) = &mut self.editor {
                    editor.show_error(err);
                }
            }
        }
    }

    /// Editor: the bake's progress, or whether the loaded bake still
    /// matches the scene; loads a bake that just finished.
    pub(super) fn check_bake(&mut self) {
        if let Some(job) = &mut self.bake_job {
            match job.finished() {
                None => {
                    let p = job.progress();
                    if let Some(editor) = &mut self.editor {
                        editor.bake_status = format!(
                            "BAKING {} {}/{} {:.0}%",
                            p.zone.to_uppercase(),
                            p.index,
                            p.count,
                            p.fraction * 100.0
                        );
                    }
                    return;
                }
                Some(result) => {
                    self.bake_job = None;
                    match result {
                        Ok(file) => {
                            log::info!("baked {}", file.display());
                            self.load_bake();
                            if let Some(editor) = &mut self.editor {
                                editor.show_message("REVERB BAKED".into());
                            }
                        }
                        Err(err) => {
                            log::error!("{err}");
                            if let Some(editor) = &mut self.editor {
                                editor.show_error(err.to_string());
                            }
                        }
                    }
                }
            }
        }
        let Some(editor) = &mut self.editor else {
            return;
        };
        editor.bake_status = match self.baked_signature {
            None => "NOT BAKED".into(),
            // Compare against the scene as edited, not as played.
            Some(baked) => {
                let scene = self.play.as_ref().map_or(&self.scene, |p| &p.scene);
                if bake::signature(scene) == baked {
                    "BAKED".into()
                } else {
                    "BAKE OUTDATED".into()
                }
            }
        };
    }

    /// Loads the scene's light bake (if any) into the renderer.
    pub(super) fn load_lightmaps(&mut self) {
        let file = lightmap::path_for(&self.scene_file());
        let baked = match lightmap::load(&file) {
            Ok(baked) => baked,
            Err(err) => {
                log::error!("{err}");
                if let Some(editor) = &mut self.editor {
                    editor.show_error(err.to_string());
                }
                None
            }
        };
        self.light_signature = baked.as_ref().map(|b| b.signature);
        self.renderer.set_lightmaps(baked.map(std::sync::Arc::new));
        self.renderer.set_scene(&self.scene, &mut self.assets);
        self.check_light_bake();
    }

    /// Editor: starts baking the scene's lighting.
    pub(super) fn start_light_bake(&mut self) {
        if self.light_job.is_some() {
            return;
        }
        // Bake the scene as edited, not as played.
        let scene = self.play.as_ref().map_or(&self.scene, |p| &p.scene);
        let file = lightmap::path_for(&self.scene_file());
        match lightmap::start(scene, &mut self.assets, file) {
            Ok(job) => self.light_job = Some(job),
            Err(err) => {
                log::error!("{err}");
                if let Some(editor) = &mut self.editor {
                    editor.show_error(err.to_string().to_uppercase());
                }
            }
        }
    }

    /// Editor: the light bake's progress or whether the loaded one still
    /// matches the scene; loads a bake that just finished.
    pub(super) fn check_light_bake(&mut self) {
        if let Some(job) = &mut self.light_job {
            match job.finished() {
                None => {
                    let p = job.progress();
                    if let Some(editor) = &mut self.editor {
                        editor.light_status =
                            format!("BAKING {} {:.0}%", p.stage, p.fraction * 100.0);
                    }
                    return;
                }
                Some(result) => {
                    self.light_job = None;
                    match result {
                        Ok(file) => {
                            log::info!("baked {}", file.display());
                            self.load_lightmaps();
                            if let Some(editor) = &mut self.editor {
                                editor.show_message("LIGHTING BAKED".into());
                            }
                        }
                        Err(err) => {
                            log::error!("{err}");
                            if let Some(editor) = &mut self.editor {
                                editor.show_error(err.to_string().to_uppercase());
                            }
                        }
                    }
                }
            }
        }
        let Some(editor) = &mut self.editor else {
            return;
        };
        editor.light_status = match self.light_signature {
            None => "NOT BAKED".into(),
            Some(baked) => {
                let scene = self.play.as_ref().map_or(&self.scene, |p| &p.scene);
                if lightmap::signature(scene) == baked {
                    "BAKED".into()
                } else {
                    "BAKE OUTDATED".into()
                }
            }
        };
    }
}
