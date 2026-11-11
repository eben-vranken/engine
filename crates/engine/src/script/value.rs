//! A script's memory (`this`) in a form that goes into a saved game:
//! plain values, lists and maps of them. Anything else a script keeps (the
//! player, an object) can't be saved and is left out.

use glam::Vec3;
use rhai::{Array, Dynamic, Map};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Value {
    Nothing,
    Bool(bool),
    Whole(i64),
    Number(f64),
    Text(String),
    Vec3(Vec3),
    List(Vec<Value>),
    Map(BTreeMap<String, Value>),
}

impl Value {
    /// The value as saved; `None` for what can't be.
    pub fn from_script(value: &Dynamic) -> Option<Self> {
        if value.is_unit() {
            return Some(Self::Nothing);
        }
        if let Ok(b) = value.as_bool() {
            return Some(Self::Bool(b));
        }
        if let Ok(i) = value.as_int() {
            return Some(Self::Whole(i));
        }
        if let Ok(f) = value.as_float() {
            return Some(Self::Number(f));
        }
        if value.is_string() || value.is_char() {
            return Some(Self::Text(value.to_string()));
        }
        if let Some(v) = value.clone().try_cast::<Vec3>() {
            return Some(Self::Vec3(v));
        }
        if let Some(list) = value.read_lock::<Array>() {
            // A list keeps its places: what can't be saved becomes nothing.
            let items = list
                .iter()
                .map(|v| Self::from_script(v).unwrap_or(Self::Nothing));
            return Some(Self::List(items.collect()));
        }
        if let Some(map) = value.read_lock::<Map>() {
            let entries = map
                .iter()
                .filter_map(|(key, v)| Some((key.to_string(), Self::from_script(v)?)));
            return Some(Self::Map(entries.collect()));
        }
        None
    }

    /// The value as scripts see it.
    pub fn to_script(&self) -> Dynamic {
        match self {
            Self::Nothing => Dynamic::UNIT,
            Self::Bool(b) => (*b).into(),
            Self::Whole(i) => (*i).into(),
            Self::Number(f) => (*f).into(),
            Self::Text(text) => text.clone().into(),
            Self::Vec3(v) => Dynamic::from(*v),
            Self::List(items) => Dynamic::from_array(items.iter().map(Self::to_script).collect()),
            Self::Map(entries) => Dynamic::from_map(
                entries
                    .iter()
                    .map(|(key, v)| (key.as_str().into(), v.to_script()))
                    .collect(),
            ),
        }
    }
}
