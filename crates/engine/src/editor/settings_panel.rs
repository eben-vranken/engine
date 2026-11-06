//! The inspector when a zone or the environment is picked instead of an
//! object.

use super::history::Change;
use super::{Editor, EditorOutput, dim, inspector, section};
use crate::scene::Scene;
use egui::{RichText, Ui};
use glam::Vec3;

impl Editor {
    /// The selected zone's or the environment's settings. Returns false if
    /// neither is selected.
    pub(super) fn settings_inspector(
        &mut self,
        ui: &mut Ui,
        scene: &mut Scene,
        output: &mut EditorOutput,
    ) -> bool {
        let Some(target) = self.settings.clone() else {
            return false;
        };
        let names = inspector::Names {
            objects: Vec::new(),
            zones: Vec::new(),
        };
        let before = scene.clone();
        match target {
            SettingsTarget::Environment => {
                let env = &mut scene.environment;
                egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        ui.label(RichText::new("ENVIRONMENT").strong());
                        ui.label(dim("THE WHOLE SCENE; ZONES CAN OVERRIDE FOG AND GRADE"));
                        section(ui, "SUN");
                        sun_angles(ui, &mut env.sun_direction);
                        section(ui, "LIGHT, FOG AND LOOK");
                        inspector::fields_except(ui, env, &names, &["sun_direction"]);
                    });
            }
            SettingsTarget::Zone(name) => {
                let Some(z) = scene.zones.iter().position(|z| z.name == name) else {
                    self.settings = None;
                    return false;
                };
                let env = scene.environment.clone();
                let zone = &mut scene.zones[z];
                egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            inspector::field_label(ui, "ZONE");
                            ui.label(zone.name.to_uppercase());
                        });
                        ui.label(dim("WITHOUT ITS OWN, A ZONE USES THE ENVIRONMENT'S"));

                        section(ui, "FOG");
                        own_toggle(ui, "OWN FOG", &mut zone.fog, &env.fog);
                        if let Some(fog) = &mut zone.fog {
                            inspector::fields(ui, fog, &names);
                        }

                        section(ui, "COLOR GRADE");
                        own_toggle(ui, "OWN GRADE", &mut zone.grade, &env.grade);
                        if let Some(grade) = &mut zone.grade {
                            inspector::fields(ui, grade, &names);
                        }
                    });
            }
        }
        if *scene != before {
            if self.zone_pending.is_none() {
                self.commit_pending(&before);
                self.zone_pending = Some(Box::new(before));
            }
            output.changed.push(Change::Settings);
        }
        true
    }
}

/// What the inspector shows when it isn't showing an object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsTarget {
    Environment,
    /// A zone, by name.
    Zone(String),
}

/// The sun's direction as compass angle and height above the horizon
/// (degrees), easier to aim than a vector.
pub(super) fn sun_angles(ui: &mut Ui, direction: &mut Vec3) {
    // The direction the light travels; the sun sits the other way.
    let to_sun = -direction.normalize_or(Vec3::NEG_Y);
    let mut azimuth = to_sun.x.atan2(-to_sun.z).to_degrees();
    let mut elevation = to_sun.y.clamp(-1.0, 1.0).asin().to_degrees();
    let mut changed = false;
    ui.horizontal(|ui| {
        inspector::field_label(ui, "AZIMUTH");
        changed |= inspector::drag(ui, &mut azimuth, 0.5, 5.0);
    });
    ui.horizontal(|ui| {
        inspector::field_label(ui, "ELEVATION");
        changed |= inspector::drag(ui, &mut elevation, 0.25, 5.0);
    });
    if changed {
        let (az, el) = (
            azimuth.to_radians(),
            elevation.clamp(-89.0, 89.0).to_radians(),
        );
        let to_sun = Vec3::new(az.sin() * el.cos(), el.sin(), -az.cos() * el.cos());
        *direction = -to_sun;
    }
}

/// [ON]/[OFF] for a zone setting: on starts from the environment's value.
pub(super) fn own_toggle<T: Clone>(ui: &mut Ui, label: &str, value: &mut Option<T>, fallback: &T) {
    ui.horizontal(|ui| {
        inspector::field_label(ui, label);
        let text = if value.is_some() { "[ON] " } else { "[OFF]" };
        if ui.small_button(text).clicked() {
            *value = match value {
                Some(_) => None,
                None => Some(fallback.clone()),
            };
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::testing::*;
    use crate::player::settings::PlayerSettings;

    #[test]
    fn sun_angles_round_trip() {
        let dir = Vec3::new(0.62, -0.62, 0.3).normalize();
        let to_sun = -dir;
        let az = to_sun.x.atan2(-to_sun.z);
        let el = to_sun.y.asin();
        let back = -Vec3::new(az.sin() * el.cos(), el.sin(), -az.cos() * el.cos());
        assert!((back - dir).length() < 1e-5);
    }
    #[test]
    fn a_zone_fog_edit_is_one_undo_step() {
        let mut scene = boxes(1);
        let mut editor = editor();
        editor.settings = Some(SettingsTarget::Zone("test".into()));
        // As the zone inspector does: remember the scene, then edit live.
        editor.zone_pending = Some(Box::new(scene.clone()));
        scene.zones[0].fog = Some(crate::scene::Fog::default());
        scene.zones[0].fog.as_mut().unwrap().density = 0.3;
        assert!(editor.is_dirty());
        assert!(editor.commit_pending(&scene));
        editor.undo(&mut scene, &mut PlayerSettings::default());
        assert_eq!(scene.zones[0].fog, None);
        // Selecting an object leaves the zone.
        editor.select(&scene, Some(0));
        assert_eq!(editor.settings, None);
    }
}
