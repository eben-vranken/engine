//! Flags: the game's named state (yes/no, numbers, text), declared with
//! starting values in the project's `flags.ron`, set and checked by
//! graphs. Abilities, story progress and dialogue outcomes are all flags:
//! an ability is a yes/no flag with a script bound to it, which runs while
//! the flag is true.

use crate::scene::{FlagRef, ScriptRef};
use bevy_reflect::Reflect;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FlagValue {
    Bool(bool),
    Number(f32),
    Text(String),
}

impl Default for FlagValue {
    fn default() -> Self {
        FlagValue::Bool(false)
    }
}

impl FlagValue {
    /// On for yes/no; not zero for numbers; not empty for text.
    pub fn is_true(&self) -> bool {
        match self {
            FlagValue::Bool(b) => *b,
            FlagValue::Number(n) => *n != 0.0,
            FlagValue::Text(t) => !t.is_empty(),
        }
    }

    pub fn as_number(&self) -> f32 {
        match self {
            FlagValue::Bool(b) => *b as u8 as f32,
            FlagValue::Number(n) => *n,
            FlagValue::Text(t) => t.trim().parse().unwrap_or(0.0),
        }
    }

    /// `text` read as a value of the same kind as this one.
    pub fn parse_like(&self, text: &str) -> FlagValue {
        let text = text.trim();
        match self {
            FlagValue::Bool(_) => FlagValue::Bool(parse_bool(text).unwrap_or(false)),
            FlagValue::Number(_) => FlagValue::Number(text.parse().unwrap_or(0.0)),
            FlagValue::Text(_) => FlagValue::Text(text.to_owned()),
        }
    }

    /// `text` read as whatever it looks like (for flags nobody declared).
    pub fn guess(text: &str) -> FlagValue {
        let text = text.trim();
        if let Some(b) = parse_bool(text) {
            FlagValue::Bool(b)
        } else if let Ok(n) = text.parse() {
            FlagValue::Number(n)
        } else {
            FlagValue::Text(text.to_owned())
        }
    }

    /// The kind's name, for the flags panel.
    pub fn kind(&self) -> &'static str {
        match self {
            FlagValue::Bool(_) => "YES/NO",
            FlagValue::Number(_) => "NUMBER",
            FlagValue::Text(_) => "TEXT",
        }
    }
}

impl std::fmt::Display for FlagValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FlagValue::Bool(b) => write!(f, "{b}"),
            FlagValue::Number(n) => write!(f, "{n}"),
            FlagValue::Text(t) => write!(f, "\"{t}\""),
        }
    }
}

fn parse_bool(text: &str) -> Option<bool> {
    match text.to_lowercase().as_str() {
        "true" | "yes" | "on" => Some(true),
        "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

/// One declared flag and the value it starts with.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlagDef {
    pub name: String,
    pub value: FlagValue,
    /// Makes the flag an ability: this script runs while it is true.
    #[serde(default, skip_serializing_if = "no_script")]
    pub script: ScriptRef,
}

fn no_script(script: &ScriptRef) -> bool {
    script.0.is_empty()
}

/// `flags.ron`: every flag the game uses, with its starting value.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct FlagDefs {
    pub flags: Vec<FlagDef>,
}

impl FlagDefs {
    pub fn file(project_dir: &Path) -> PathBuf {
        project_dir.join("flags.ron")
    }

    /// The project's flags; none if it has no `flags.ron` (or it doesn't
    /// read, which is logged).
    pub fn load(project_dir: &Path) -> Self {
        let path = Self::file(project_dir);
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        ron::from_str(&text).unwrap_or_else(|err| {
            log::error!("{}: {err}", path.display());
            Self::default()
        })
    }

    pub fn save(&self, project_dir: &Path) -> std::io::Result<()> {
        let config = ron::ser::PrettyConfig::default().struct_names(false);
        let text = ron::ser::to_string_pretty(self, config).expect("flags always serialize");
        std::fs::write(Self::file(project_dir), text)
    }

    /// Abilities: script path → the flag that turns it on.
    pub fn abilities(&self) -> BTreeMap<String, String> {
        self.flags
            .iter()
            .filter(|f| !f.script.0.is_empty())
            .map(|f| (f.script.0.clone(), f.name.clone()))
            .collect()
    }

    pub fn names(&self) -> Vec<String> {
        self.flags.iter().map(|f| f.name.clone()).collect()
    }

    /// The flags as the game starts.
    pub fn start(&self) -> Flags {
        Flags(
            self.flags
                .iter()
                .map(|f| (f.name.clone(), f.value.clone()))
                .collect(),
        )
    }
}

/// The flags' current values.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Flags(pub BTreeMap<String, FlagValue>);

impl Flags {
    /// A flag's value; unknown flags are off.
    pub fn get(&self, name: &str) -> FlagValue {
        self.0.get(name).cloned().unwrap_or_default()
    }

    pub fn is_true(&self, name: &str) -> bool {
        self.0.get(name).is_some_and(FlagValue::is_true)
    }

    /// Sets a flag from text, read as the flag's kind.
    pub fn set(&mut self, name: &str, text: &str) {
        if name.is_empty() {
            return;
        }
        let value = match self.0.get(name) {
            Some(current) => current.parse_like(text),
            None => FlagValue::guess(text),
        };
        self.0.insert(name.to_owned(), value);
    }

    /// Adds to a number flag (other kinds become numbers).
    pub fn add(&mut self, name: &str, amount: f32) {
        if name.is_empty() {
            return;
        }
        let value = self.get(name).as_number() + amount;
        self.0.insert(name.to_owned(), FlagValue::Number(value));
    }
}

/// A test on a flag. An empty flag always holds.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, Reflect)]
#[serde(default)]
pub struct Condition {
    pub flag: FlagRef,
    pub test: Test,
    /// Compared against for EQUALS, AT LEAST and so on.
    pub value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, Reflect)]
pub enum Test {
    #[default]
    IsTrue,
    IsFalse,
    Equals,
    NotEquals,
    AtLeast,
    AtMost,
}

impl Condition {
    pub fn holds(&self, flags: &Flags) -> bool {
        if self.flag.0.is_empty() {
            return true;
        }
        let current = flags.get(&self.flag.0);
        match self.test {
            Test::IsTrue => current.is_true(),
            Test::IsFalse => !current.is_true(),
            Test::Equals | Test::NotEquals => {
                let equal = match (&current, current.parse_like(&self.value)) {
                    (FlagValue::Number(a), FlagValue::Number(b)) => (a - b).abs() < 1e-4,
                    (a, b) => *a == b,
                };
                equal == (self.test == Test::Equals)
            }
            Test::AtLeast => current.as_number() >= self.value.trim().parse().unwrap_or(0.0),
            Test::AtMost => current.as_number() <= self.value.trim().parse().unwrap_or(0.0),
        }
    }

    /// `has_lantern IS TRUE`, `coins AT LEAST 3`.
    pub fn describe(&self) -> String {
        if self.flag.0.is_empty() {
            return "ALWAYS".into();
        }
        let test = match self.test {
            Test::IsTrue => return format!("{} IS TRUE", self.flag.0),
            Test::IsFalse => return format!("{} IS FALSE", self.flag.0),
            Test::Equals => "=",
            Test::NotEquals => "≠",
            Test::AtLeast => "≥",
            Test::AtMost => "≤",
        };
        format!("{} {test} {}", self.flag.0, self.value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flags() -> Flags {
        FlagDefs {
            flags: vec![
                FlagDef {
                    name: "lantern".into(),
                    value: FlagValue::Bool(false),
                    script: ScriptRef::default(),
                },
                FlagDef {
                    name: "coins".into(),
                    value: FlagValue::Number(0.0),
                    script: ScriptRef::default(),
                },
                FlagDef {
                    name: "path".into(),
                    value: FlagValue::Text(String::new()),
                    script: ScriptRef::default(),
                },
            ],
        }
        .start()
    }

    fn condition(flag: &str, test: Test, value: &str) -> Condition {
        Condition {
            flag: FlagRef(flag.into()),
            test,
            value: value.into(),
        }
    }

    #[test]
    fn flags_keep_their_kind() {
        let mut flags = flags();
        flags.set("lantern", "yes");
        flags.set("coins", "2");
        flags.add("coins", 1.5);
        flags.set("path", "north");
        flags.set("undeclared", "4");
        assert_eq!(flags.get("lantern"), FlagValue::Bool(true));
        assert_eq!(flags.get("coins"), FlagValue::Number(3.5));
        assert_eq!(flags.get("path"), FlagValue::Text("north".into()));
        assert_eq!(flags.get("undeclared"), FlagValue::Number(4.0));
        assert_eq!(flags.get("nothing"), FlagValue::Bool(false));
    }

    #[test]
    fn conditions_test_flags() {
        let mut flags = flags();
        assert!(Condition::default().holds(&flags), "no flag: always");
        assert!(condition("lantern", Test::IsFalse, "").holds(&flags));
        flags.set("lantern", "true");
        assert!(condition("lantern", Test::IsTrue, "").holds(&flags));
        flags.set("coins", "3");
        assert!(condition("coins", Test::AtLeast, "3").holds(&flags));
        assert!(!condition("coins", Test::AtMost, "2").holds(&flags));
        assert!(condition("coins", Test::Equals, "3.0").holds(&flags));
        flags.set("path", "north");
        assert!(condition("path", Test::Equals, "north").holds(&flags));
        assert!(condition("path", Test::NotEquals, "south").holds(&flags));
        assert_eq!(
            condition("coins", Test::AtLeast, "3").describe(),
            "coins ≥ 3"
        );
    }
}
