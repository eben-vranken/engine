//! The inspector when a zone or the environment is picked instead of an
//! object.

use super::history::Change;
use super::{Editor, EditorOutput, dim, inspector, section};
use crate::scene::{Component, Environment, Scene, ZoneReverb};
use crate::ui::theme;
use egui::{Align, Layout, RichText, Ui};
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
        let names = inspector::Names::default();
        let before = scene.clone();
        match target {
            SettingsTarget::Environment => {
                let env = &mut scene.environment;
                let light_status = self.light_status.as_str();
                egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        ui.label(RichText::new("ENVIRONMENT").strong());
                        ui.label(dim("THE WHOLE SCENE; ZONES CAN OVERRIDE FOG AND GRADE"));
                        bake_note(ui, light_status);

                        section(ui, "SUN");
                        let mut on = env.sun_color != Vec3::ZERO;
                        if on_off(ui, "SUN", &mut on) {
                            env.sun_color = if on {
                                Environment::default().sun_color
                            } else {
                                Vec3::ZERO
                            };
                        }
                        if on {
                            sun_angles(ui, &mut env.sun_direction);
                            vec3_row(ui, "COLOR", &mut env.sun_color);
                            ui.label(dim("LINEAR RGB × STRENGTH; HIGHER = BRIGHTER"));
                        }

                        section(ui, "SKY LIGHT");
                        vec3_row(ui, "SKY", &mut env.ambient_sky);
                        vec3_row(ui, "GROUND", &mut env.ambient_ground);
                        ui.label(dim(
                            "LIGHT FROM ALL AROUND: THE SKY ABOVE, THE GROUND BELOW. 0 0 0 = NONE",
                        ));

                        section(ui, "FOG AND LOOK");
                        inspector::fields_except(
                            ui,
                            env,
                            &names,
                            &[
                                "sun_direction",
                                "sun_color",
                                "ambient_sky",
                                "ambient_ground",
                            ],
                        );
                    });
            }
            SettingsTarget::Zone(name) => {
                let Some(z) = scene.zones.iter().position(|z| z.name == name) else {
                    self.settings = None;
                    return false;
                };
                let env = scene.environment.clone();
                let zone = &mut scene.zones[z];
                let objects = &mut scene.objects;
                let mut pick = None;
                egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            inspector::field_label(ui, "ZONE");
                            ui.label(zone.name.to_uppercase());
                        });
                        ui.label(dim("WITHOUT ITS OWN, A ZONE USES THE ENVIRONMENT'S"));

                        // The zone's space is its volume objects.
                        section(ui, "VOLUME");
                        let mut any = false;
                        for (index, object) in objects
                            .iter_mut()
                            .enumerate()
                            .filter(|(_, o)| o.zone == zone.name)
                        {
                            let Some(volume) = object.components.iter_mut().find_map(|c| match c {
                                Component::ZoneVolume(v) => Some(v),
                                _ => None,
                            }) else {
                                continue;
                            };
                            any = true;
                            ui.horizontal(|ui| {
                                ui.label(object.name.to_uppercase());
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if ui
                                        .small_button("SELECT")
                                        .on_hover_text("MOVE, TURN OR SCALE IT WITH THE GIZMO")
                                        .clicked()
                                    {
                                        pick = Some(index);
                                    }
                                });
                            });
                            inspector::fields(ui, volume, &names);
                            ui.add_space(4.0);
                        }
                        if !any {
                            ui.label(dim("NONE: ADD ▸ ZONE VOLUME GIVES THE ZONE ITS SPACE"));
                        }

                        section(ui, "FOG");
                        own_toggle(ui, "OWN FOG", &mut zone.fog, &env.fog);
                        if let Some(fog) = &mut zone.fog {
                            inspector::fields(ui, fog, &names);
                        }

                        section(ui, "REVERB");
                        own_toggle(ui, "OWN REVERB", &mut zone.reverb, &ZoneReverb::default());
                        match &mut zone.reverb {
                            Some(reverb) => {
                                inspector::fields(ui, reverb, &names);
                                ui.label(dim(
                                    "A TAIL ON EVERY SOUND WHILE YOU'RE IN THIS ZONE, WALLS OR NOT",
                                ));
                            }
                            None => {
                                ui.label(dim("ONLY THE SIMULATED ROOM'S (NONE IN OPEN SPACE)"));
                            }
                        }

                        section(ui, "COLOR GRADE");
                        own_toggle(ui, "OWN GRADE", &mut zone.grade, &env.grade);
                        if let Some(grade) = &mut zone.grade {
                            inspector::fields(ui, grade, &names);
                        }
                    });
                if let Some(index) = pick {
                    self.select(scene, Some(index));
                    self.reveal_selection = true;
                }
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

/// A light bake keeps the sky, sun and bounce light it was made with;
/// says so while one is loaded.
fn bake_note(ui: &mut Ui, status: &str) {
    if status.is_empty() || status == "NOT BAKED" {
        return;
    }
    let text = "THE LIGHT BAKE KEEPS THE SKY AND SUN LIGHT IT WAS MADE WITH:                 BAKE ▸ LIGHTING AGAIN TO SEE CHANGES TO THEM";
    let color = if status.contains("OUTDATED") {
        theme::ALERT
    } else {
        theme::AMBER_DIM
    };
    ui.label(RichText::new(text).small().color(color));
}

/// A row of three numbers (a color or direction).
fn vec3_row(ui: &mut Ui, label: &str, value: &mut Vec3) {
    ui.horizontal(|ui| {
        inspector::field_label(ui, label);
        for axis in 0..3 {
            let mut v = value[axis];
            if inspector::drag(ui, &mut v, 0.01, 0.1) {
                value[axis] = v.max(0.0);
            }
        }
    });
}

/// [ON]/[OFF] row; returns true if it was flipped.
fn on_off(ui: &mut Ui, label: &str, value: &mut bool) -> bool {
    let before = *value;
    super::widgets::on_off(ui, label, value);
    *value != before
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
