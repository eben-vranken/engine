//! Inspector widgets generated from reflection: any component that derives
//! `Reflect` gets an editable panel without UI code of its own. Numbers become
//! drag fields, vectors three fields in a row, booleans toggles, text a text
//! box; nested structs and enum fields are indented underneath.

use crate::scene::{CharacterRef, FlagRef, GraphRef, ObjectRef, SceneRef, SoundRef, ZoneRef};
use crate::ui::theme;
use bevy_reflect::enums::{DynamicEnum, DynamicVariant, VariantInfo};
use bevy_reflect::{PartialReflect, ReflectMut, ReflectRef, TypeInfo};
use egui::{DragValue, Ui};
use glam::{Vec2, Vec3};

const LABEL_WIDTH: f32 = 96.0;

/// What reference pickers offer: object and zone names, and the
/// project's flags, graphs, scenes, sounds and character ids.
#[derive(Debug, Clone, Default)]
pub struct Names {
    pub objects: Vec<String>,
    pub zones: Vec<String>,
    pub flags: Vec<String>,
    pub graphs: Vec<String>,
    pub scenes: Vec<String>,
    pub sounds: Vec<String>,
    pub characters: Vec<String>,
}

/// Draws one row per field of a struct or enum value. Returns true if
/// anything changed.
pub fn fields(ui: &mut Ui, value: &mut dyn PartialReflect, names: &Names) -> bool {
    fields_except(ui, value, names, &[])
}

/// Like [`fields`], leaving out the struct fields named in `skip` (for
/// values drawn with their own controls).
pub fn fields_except(
    ui: &mut Ui,
    value: &mut dyn PartialReflect,
    names: &Names,
    skip: &[&str],
) -> bool {
    let mut changed = false;
    match value.reflect_mut() {
        ReflectMut::Struct(s) => {
            for i in 0..s.field_len() {
                let name = s.name_at(i).unwrap_or_default().to_owned();
                if skip.contains(&name.as_str()) {
                    continue;
                }
                if let Some(field) = s.field_at_mut(i) {
                    changed |= row(ui, &name, field, names);
                }
            }
        }
        ReflectMut::Enum(e) => {
            for i in 0..e.field_len() {
                let name = e.name_at(i).map(str::to_owned);
                let Some(field) = e.field_at_mut(i) else {
                    continue;
                };
                match name {
                    Some(name) => changed |= row(ui, &name, field, names),
                    // A tuple variant wrapping one struct: show the struct's fields.
                    None => changed |= fields(ui, field, names),
                }
            }
        }
        _ => {}
    }
    changed
}

/// Copies what changed from `before` to `after` into `target`, field by
/// field, leaving `target`'s other fields as they are (editing several
/// objects at once). Nested structs and enums of the same variant are
/// compared inside; anything else that changed is copied whole.
pub fn copy_changes(
    before: &dyn PartialReflect,
    after: &dyn PartialReflect,
    target: &mut dyn PartialReflect,
) {
    if before.reflect_partial_eq(after) == Some(true) {
        return;
    }
    match (
        before.reflect_ref(),
        after.reflect_ref(),
        target.reflect_mut(),
    ) {
        (ReflectRef::Struct(b), ReflectRef::Struct(a), ReflectMut::Struct(t)) => {
            for i in 0..a.field_len() {
                let Some(name) = a.name_at(i) else { continue };
                if let (Some(bf), Some(af), Some(tf)) =
                    (b.field(name), a.field_at(i), t.field_mut(name))
                {
                    copy_changes(bf, af, tf);
                }
            }
        }
        (ReflectRef::Enum(b), ReflectRef::Enum(a), ReflectMut::Enum(t))
            if b.variant_name() == a.variant_name() && t.variant_name() == a.variant_name() =>
        {
            for i in 0..a.field_len() {
                if let (Some(bf), Some(af), Some(tf)) =
                    (b.field_at(i), a.field_at(i), t.field_at_mut(i))
                {
                    copy_changes(bf, af, tf);
                }
            }
        }
        _ => {
            if let Err(err) = target.try_apply(after) {
                log::warn!("couldn't copy a change: {err}");
            }
        }
    }
}

/// The section title for a value: an enum shows its variant name.
pub fn title(value: &dyn PartialReflect) -> String {
    match value.reflect_ref() {
        ReflectRef::Enum(e) => label_text(e.variant_name()),
        _ => value
            .get_represented_type_info()
            .map(|info| label_text(info.type_path_table().ident().unwrap_or("?")))
            .unwrap_or_default(),
    }
}

fn row(ui: &mut Ui, name: &str, field: &mut dyn PartialReflect, names: &Names) -> bool {
    let speed = drag_speed(name);
    let step = snap_step(name);
    let label = |ui: &mut Ui| field_label(ui, &label_text(name));

    if let Some(v) = field.try_downcast_mut::<f32>() {
        return ui
            .horizontal(|ui| {
                label(ui);
                drag(ui, v, speed, step)
            })
            .inner;
    }
    if let Some(v) = field.try_downcast_mut::<Vec3>() {
        return ui
            .horizontal(|ui| {
                label(ui);
                let mut changed = false;
                for c in [&mut v.x, &mut v.y, &mut v.z] {
                    changed |= drag(ui, c, speed, step);
                }
                changed
            })
            .inner;
    }
    if let Some(v) = field.try_downcast_mut::<Vec2>() {
        return ui
            .horizontal(|ui| {
                label(ui);
                let mut changed = false;
                for c in [&mut v.x, &mut v.y] {
                    changed |= drag(ui, c, speed, step);
                }
                changed
            })
            .inner;
    }
    if let Some(v) = field.try_downcast_mut::<u32>() {
        return ui
            .horizontal(|ui| {
                label(ui);
                ui.add(DragValue::new(v).speed(0.05)).changed()
            })
            .inner;
    }
    if let Some(v) = field.try_downcast_mut::<bool>() {
        return ui
            .horizontal(|ui| {
                label(ui);
                let text = if *v { "[ON] " } else { "[OFF]" };
                let clicked = ui.small_button(text).clicked();
                if clicked {
                    *v = !*v;
                }
                clicked
            })
            .inner;
    }
    if let Some(v) = field.try_downcast_mut::<ObjectRef>() {
        return ui
            .horizontal(|ui| {
                label(ui);
                reference_picker(ui, name, &mut v.0, &names.objects)
            })
            .inner;
    }
    if let Some((value, list)) = reference(field, names) {
        return ui
            .horizontal(|ui| {
                label(ui);
                reference_picker(ui, name, value, list)
            })
            .inner;
    }
    // Dialogue and notes: several lines.
    if name == "text"
        && let Some(v) = field.try_downcast_mut::<String>()
    {
        label(ui);
        return ui
            .add(
                egui::TextEdit::multiline(v)
                    .desired_rows(3)
                    .desired_width(f32::INFINITY),
            )
            .changed();
    }
    if let Some(v) = field.try_downcast_mut::<String>() {
        return ui
            .horizontal(|ui| {
                label(ui);
                ui.text_edit_singleline(v).changed()
            })
            .inner;
    }

    if let Some(variants) = unit_variants(field) {
        return ui
            .horizontal(|ui| {
                label(ui);
                variant_picker(ui, name, field, &variants)
            })
            .inner;
    }

    // Nested struct or enum: a heading row, its fields indented below.
    ui.horizontal(|ui| {
        label(ui);
        if let bevy_reflect::ReflectRef::Enum(e) = field.reflect_ref() {
            ui.label(label_text(e.variant_name()));
        }
    });
    ui.indent(name, |ui| fields(ui, field, names)).inner
}

/// A reference to something by name (a zone, a flag, a graph...): its
/// text and the names to pick from.
fn reference<'a>(
    field: &'a mut dyn PartialReflect,
    names: &'a Names,
) -> Option<(&'a mut String, &'a [String])> {
    macro_rules! pick {
        ($ty:ty, $list:ident) => {
            if field.try_downcast_ref::<$ty>().is_some() {
                return field
                    .try_downcast_mut::<$ty>()
                    .map(|v| (&mut v.0, names.$list.as_slice()));
            }
        };
    }
    pick!(ZoneRef, zones);
    pick!(FlagRef, flags);
    pick!(GraphRef, graphs);
    pick!(SceneRef, scenes);
    pick!(SoundRef, sounds);
    pick!(CharacterRef, characters);
    None
}

/// The variant names of an enum whose variants carry no data (a plain
/// choice, like a material preset); `None` for anything else.
fn unit_variants(field: &dyn PartialReflect) -> Option<Vec<&'static str>> {
    let TypeInfo::Enum(info) = field.get_represented_type_info()? else {
        return None;
    };
    info.iter()
        .map(|variant| match variant {
            VariantInfo::Unit(unit) => Some(unit.name()),
            _ => None,
        })
        .collect()
}

/// A dropdown of an enum's variants.
fn variant_picker(
    ui: &mut Ui,
    name: &str,
    field: &mut dyn PartialReflect,
    variants: &[&'static str],
) -> bool {
    let bevy_reflect::ReflectRef::Enum(current) = field.reflect_ref() else {
        return false;
    };
    let current = current.variant_name().to_owned();
    let mut picked = None;
    egui::ComboBox::from_id_salt(("variant", name))
        .selected_text(label_text(&current))
        .show_ui(ui, |ui| {
            for &variant in variants {
                if ui
                    .selectable_label(variant == current, label_text(variant))
                    .clicked()
                    && variant != current
                {
                    picked = Some(variant);
                }
            }
        });
    let Some(variant) = picked else {
        return false;
    };
    field
        .try_apply(&DynamicEnum::new(variant, DynamicVariant::Unit))
        .is_ok()
}

/// A dropdown of names (objects or zones); a name that no longer exists
/// shows as missing.
fn reference_picker(ui: &mut Ui, name: &str, value: &mut String, names: &[String]) -> bool {
    let current = &*value;
    let text = if current.is_empty() {
        "NONE".to_owned()
    } else if names.contains(current) {
        current.clone()
    } else {
        format!("{current} (MISSING)")
    };
    let mut picked = None;
    egui::ComboBox::from_id_salt(("reference", name))
        .selected_text(text)
        .height(320.0)
        .show_ui(ui, |ui| {
            if ui.selectable_label(current.is_empty(), "NONE").clicked() {
                picked = Some(String::new());
            }
            for option in names {
                if ui.selectable_label(option == current, option).clicked() {
                    picked = Some(option.clone());
                }
            }
        });
    match picked {
        Some(name) if name != *value => {
            *value = name;
            true
        }
        _ => false,
    }
}

/// A left-aligned, fixed-width label so the values line up in a column.
pub fn field_label(ui: &mut Ui, text: &str) {
    let size = egui::vec2(LABEL_WIDTH, ui.spacing().interact_size.y);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    ui.painter().text(
        rect.left_center(),
        egui::Align2::LEFT_CENTER,
        text,
        egui::TextStyle::Body.resolve(ui.style()),
        theme::AMBER_DIM,
    );
}

/// `max_slope` → `MAX SLOPE`.
fn label_text(name: &str) -> String {
    name.replace('_', " ").to_uppercase()
}

/// A drag field that snaps to multiples of `step` while Ctrl is held.
///
/// The unsnapped value is kept for the length of the drag, so small mouse
/// movements still add up instead of being rounded away every frame.
pub fn drag(ui: &mut Ui, value: &mut f32, speed: f64, step: f32) -> bool {
    let old = *value;
    let response = ui.add(DragValue::new(value).speed(speed).max_decimals(3));
    let raw_id = response.id.with("snap_raw");
    let snapping = ui.input(|i| i.modifiers.ctrl);
    if response.dragged() && snapping {
        let raw = ui.data(|d| d.get_temp::<f32>(raw_id)).unwrap_or(old) + (*value - old);
        ui.data_mut(|d| d.insert_temp(raw_id, raw));
        *value = snap(raw, step);
    } else {
        ui.data_mut(|d| d.remove::<f32>(raw_id));
    }
    *value != old
}

/// Rounds to the nearest multiple of `step`.
pub fn snap(value: f32, step: f32) -> f32 {
    (value / step).round() * step + 0.0
}

/// Snap increment while Ctrl is held: grid lines for distances, 15° for
/// angles.
pub fn snap_step(name: &str) -> f32 {
    if name.contains("rotation") || name.contains("angle") {
        15.0
    } else if name.contains("scale") {
        0.1
    } else if name.contains("color") {
        0.05
    } else {
        0.25
    }
}

/// Degrees move faster per pixel than meters.
fn drag_speed(name: &str) -> f64 {
    if name.contains("rotation") || name.contains("angle") {
        0.5
    } else {
        0.01
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn copy_changes_copies_only_what_changed() {
        use crate::scene::{LightSource, ModelPart};
        let before = ModelPart::default();
        let mut after = before.clone();
        after.emissive.y = 2.0;
        after.light = Some(LightSource::default());
        let mut target = ModelPart {
            name: "flame".into(),
            color: glam::Vec3::new(0.1, 0.2, 0.3),
            emissive: glam::Vec3::new(5.0, 0.0, 7.0),
            ..Default::default()
        };
        super::copy_changes(&before, &after, &mut target);
        assert_eq!(target.name, "flame");
        assert_eq!(target.color, glam::Vec3::new(0.1, 0.2, 0.3));
        assert_eq!(
            target.emissive,
            glam::Vec3::new(5.0, 2.0, 7.0),
            "only y changed"
        );
        assert_eq!(target.light, Some(LightSource::default()));

        // Inside Some(light): only the flicker.
        let before = target.clone();
        let mut after = target.clone();
        after.light.as_mut().unwrap().flicker = 0.8;
        let mut other = target.clone();
        other.light.as_mut().unwrap().range = 2.0;
        super::copy_changes(&before, &after, &mut other);
        let light = other.light.unwrap();
        assert_eq!((light.flicker, light.range), (0.8, 2.0));
    }

    use super::*;
    use crate::render::mesh::Shape;
    use crate::scene::AcousticPreset;

    #[test]
    fn plain_enums_become_pickers_and_switch_variants() {
        let mut preset = AcousticPreset::Wood;
        let variants = unit_variants(&preset).expect("all variants are plain");
        assert_eq!(variants.len(), 13);
        assert!(variants.contains(&"Marble"));
        preset
            .try_apply(&DynamicEnum::new("Marble", DynamicVariant::Unit))
            .unwrap();
        assert_eq!(preset, AcousticPreset::Marble);

        let shape = Shape::Box { size: Vec3::ONE };
        assert!(
            unit_variants(&shape).is_none(),
            "variants with data aren't a plain choice"
        );
    }

    #[test]
    fn snaps_to_steps() {
        assert_eq!(snap(1.12, 0.25), 1.0);
        assert_eq!(snap(1.13, 0.25), 1.25);
        assert_eq!(snap(-0.1, 0.25), 0.0);
        assert_eq!(snap(22.0, 15.0), 15.0);
        assert_eq!(snap(23.0, 15.0), 30.0);
    }

    #[test]
    fn steps_follow_field_names() {
        assert_eq!(snap_step("position"), 0.25);
        assert_eq!(snap_step("rotation"), 15.0);
        assert_eq!(snap_step("scale"), 0.1);
    }
}
