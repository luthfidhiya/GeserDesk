// SPDX-License-Identifier: GPL-3.0-or-later
//! This machine's own setup of its special keys: `keys.toml` in the user's config
//! folder, next to `gui.toml`.
//!
//! It belongs to the machine and is read whichever part the machine plays, so a
//! computer that was a client yesterday and is the server today uses the same file:
//!
//! ```toml
//! # What this machine's keyboard means (read when it is the server).
//! [capture]
//! "ctrl+alt+f5" = "brightness_down"
//! "ctrl+alt+f4" = "mic_mute"
//!
//! # How this machine does an action it is sent (read when it is a client).
//! [perform]
//! brightness_up = "ignore"
//! mic_mute = "keysym:0x1008ffb2"
//! ```
//!
//! Only what differs from the built-in setup needs to be there, and a line that cannot
//! be understood is skipped with a warning: a typo costs that line, not the machine its
//! keyboard.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use geserdesk_proto::consumer;
use geserdesk_proto::keys::{Perform, Rule};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::settings::config_dir;

/// What the file says, once understood.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct KeyConfig {
    /// Key combinations on this machine's keyboard that mean an action.
    pub rules: Vec<Rule>,
    /// How this machine does an action, by the action's code.
    pub perform: HashMap<u16, Perform>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Raw {
    capture: BTreeMap<String, String>,
    perform: BTreeMap<String, String>,
}

/// Where the file is.
pub fn path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("keys.toml"))
}

/// Read this machine's `keys.toml`. A missing file is the built-in setup; a file or a line
/// that cannot be understood is reported in the log and skipped.
pub fn load() -> KeyConfig {
    let Some(path) = path() else {
        return KeyConfig::default();
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return KeyConfig::default(),
        Err(e) => {
            warn!(file = %path.display(), error = %e, "could not read keys.toml; using the built-in keys");
            return KeyConfig::default();
        }
    };
    let (config, problems) = parse(&text);
    for p in problems {
        warn!(file = %path.display(), "keys.toml: {p}");
    }
    config
}

/// Understand `text`: the setup, and what was wrong with each line left out.
pub fn parse(text: &str) -> (KeyConfig, Vec<String>) {
    let raw: Raw = match toml::from_str(text) {
        Ok(raw) => raw,
        Err(e) => return (KeyConfig::default(), vec![e.to_string()]),
    };
    let mut config = KeyConfig::default();
    let mut problems = Vec::new();
    for (trigger, action) in &raw.capture {
        match Rule::parse(trigger, action) {
            Ok(rule) => config.rules.push(rule),
            Err(e) => problems.push(format!("[capture] {trigger}: {e}")),
        }
    }
    for (action, way) in &raw.perform {
        let Some(hid) = consumer::from_name(action) else {
            problems.push(format!("[perform] {action}: unknown action"));
            continue;
        };
        match Perform::parse(way) {
            Ok(perform) => {
                config.perform.insert(hid, perform);
            }
            Err(e) => problems.push(format!("[perform] {action}: {e}")),
        }
    }
    (config, problems)
}

/// An action the window can set up, as the code knows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ActionInfo {
    /// The name `keys.toml` uses.
    pub name: &'static str,
    /// Whether this machine does it with nothing set up.
    pub built_in: bool,
}

/// Every action, in the order the code lists them.
pub fn actions() -> Vec<ActionInfo> {
    consumer::ACTIONS
        .iter()
        .map(|&(name, hid)| ActionInfo {
            name,
            built_in: geserdesk_client::has_built_in(hid),
        })
        .collect()
}

/// A key combination and the action it means, as written in the file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleEntry {
    pub trigger: String,
    pub action: String,
}

/// An action and how this machine does it, as written in the file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WayEntry {
    pub action: String,
    pub way: String,
}

/// The two tables of `keys.toml`, as the window shows and edits them.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyEntries {
    pub rules: Vec<RuleEntry>,
    pub ways: Vec<WayEntry>,
    /// What the machine would skip in the file as it is, line by line (read only; the
    /// window shows it so a hand-made mistake is seen before the next Start).
    #[serde(default, skip_deserializing)]
    pub problems: Vec<String>,
}

/// The entries of `keys.toml` as written, without judging them (a line that cannot be
/// understood is still shown, so it can be corrected).
pub fn read_entries() -> Result<KeyEntries, String> {
    let Some(path) = path() else {
        return Ok(KeyEntries::default());
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => entries_of(&text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(KeyEntries::default()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

fn entries_of(text: &str) -> Result<KeyEntries, String> {
    let raw: Raw = toml::from_str(text).map_err(|e| format!("keys.toml: {e}"))?;
    let problems = parse(text).1;
    Ok(KeyEntries {
        problems,
        rules: raw
            .capture
            .into_iter()
            .map(|(trigger, action)| RuleEntry { trigger, action })
            .collect(),
        ways: raw
            .perform
            .into_iter()
            .map(|(action, way)| WayEntry { action, way })
            .collect(),
    })
}

/// Check every entry with the parsers the machine reads the file with, naming the first
/// that is wrong.
pub fn check(entries: &KeyEntries) -> Result<(), String> {
    let mut seen = std::collections::HashSet::new();
    for r in &entries.rules {
        let rule = Rule::parse(&r.trigger, &r.action).map_err(|e| format!("{}: {e}", r.trigger))?;
        if !seen.insert((rule.mods.bits(), rule.hid)) {
            return Err(format!("{}: the combination is used twice", r.trigger));
        }
    }
    let mut actions = std::collections::HashSet::new();
    for w in &entries.ways {
        if consumer::from_name(&w.action).is_none() {
            return Err(format!("{}: unknown action", w.action));
        }
        if !actions.insert(w.action.as_str()) {
            return Err(format!("{}: the action is set twice", w.action));
        }
        Perform::parse(&w.way).map_err(|e| format!("{}: {e}", w.action))?;
    }
    Ok(())
}

/// Save `entries` as the machine's `keys.toml`.
pub fn write_entries(entries: &KeyEntries) -> Result<(), String> {
    let path = path().ok_or("no config folder on this machine")?;
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let out = edited(&text, entries)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(&path, out).map_err(|e| format!("{}: {e}", path.display()))
}

/// `text` with `[capture]` and `[perform]` replaced by `entries`; comments and any other
/// content stay. An empty table is removed.
fn edited(text: &str, entries: &KeyEntries) -> Result<String, String> {
    check(entries)?;
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("keys.toml: {e}"))?;
    let mut put = |name: &str, pairs: Vec<(&str, &str)>| {
        if pairs.is_empty() {
            doc.remove(name);
            return;
        }
        let mut table = toml_edit::Table::new();
        for (k, v) in pairs {
            table.insert(k, toml_edit::value(v));
        }
        doc[name] = toml_edit::Item::Table(table);
    };
    put(
        "capture",
        entries
            .rules
            .iter()
            .map(|r| (r.trigger.trim(), r.action.trim()))
            .collect(),
    );
    put(
        "perform",
        entries
            .ways
            .iter()
            .map(|w| (w.action.trim(), w.way.trim()))
            .collect(),
    );
    Ok(doc.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use geserdesk_proto::ModMask;

    #[test]
    fn rules_and_ways_are_read_and_a_bad_line_costs_only_itself() {
        let (cfg, problems) = parse(
            "[capture]\n\"ctrl+alt+f5\" = \"brightness_down\"\n\"ctrl+banana\" = \"mute\"\n\
             [perform]\nbrightness_up = \"ignore\"\nmic_mute = \"keysym:0x1008ffb2\"\nnope = \"ignore\"\nmute = \"press x\"\n",
        );
        assert_eq!(cfg.rules.len(), 1);
        assert_eq!(cfg.rules[0].mods, ModMask::CONTROL | ModMask::ALT);
        assert_eq!(cfg.rules[0].action, consumer::BRIGHTNESS_DOWN);
        assert_eq!(cfg.perform[&consumer::BRIGHTNESS_UP], Perform::Ignore);
        assert_eq!(
            cfg.perform[&consumer::MIC_MUTE],
            Perform::Keysym(0x1008_FFB2)
        );
        assert_eq!(cfg.perform.len(), 2);
        assert_eq!(problems.len(), 3, "{problems:?}");
        assert!(problems.iter().any(|p| p.contains("ctrl+banana")));
        assert!(problems.iter().any(|p| p.contains("nope")));
        assert!(problems.iter().any(|p| p.contains("mute")));
    }

    #[test]
    fn an_empty_file_or_one_with_other_sections_is_the_built_in_setup() {
        assert_eq!(parse("").0, KeyConfig::default());
        let (cfg, problems) = parse("[something_else]\nx = 1\n");
        assert_eq!(cfg, KeyConfig::default());
        assert!(problems.is_empty());
    }

    #[test]
    fn a_file_that_is_not_toml_is_one_problem_and_no_setup() {
        let (cfg, problems) = parse("[capture\n");
        assert_eq!(cfg, KeyConfig::default());
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn saving_replaces_the_two_tables_and_keeps_the_rest() {
        let e = KeyEntries {
            rules: vec![RuleEntry {
                trigger: "ctrl+alt+f5".into(),
                action: "brightness_down".into(),
            }],
            ways: vec![WayEntry {
                action: "mic_mute".into(),
                way: "ignore".into(),
            }],
            problems: vec![],
        };
        let out = edited("# mine\n[other]\nx = 1\n[capture]\n\"f1\" = \"mute\"\n", &e).unwrap();
        assert!(out.contains("# mine") && out.contains("x = 1"));
        assert!(!out.contains("\"f1\""));
        assert_eq!(entries_of(&out).unwrap(), e);
        let bad = entries_of("[capture]\n\"ctrl+banana\" = \"mute\"\n").unwrap();
        assert_eq!(
            bad.rules.len(),
            1,
            "a bad line is still shown, to be corrected"
        );
        assert!(bad.problems[0].contains("ctrl+banana"));
        let gone = edited(&out, &KeyEntries::default()).unwrap();
        assert!(!gone.contains("capture") && !gone.contains("perform") && gone.contains("x = 1"));
    }

    #[test]
    fn a_wrong_entry_is_refused_by_name() {
        let bad = |t: &str, a: &str| KeyEntries {
            rules: vec![RuleEntry {
                trigger: t.into(),
                action: a.into(),
            }],
            ways: vec![],
            problems: vec![],
        };
        assert!(check(&bad("ctrl+banana", "mute"))
            .unwrap_err()
            .contains("ctrl+banana"));
        let twice = KeyEntries {
            rules: vec![
                RuleEntry {
                    trigger: "ctrl+f5".into(),
                    action: "mute".into(),
                },
                RuleEntry {
                    trigger: "Ctrl+F5".into(),
                    action: "stop".into(),
                },
            ],
            ways: vec![],
            problems: vec![],
        };
        assert!(check(&twice).unwrap_err().contains("twice"));
        let way = KeyEntries {
            rules: vec![],
            ways: vec![WayEntry {
                action: "mute".into(),
                way: "press x".into(),
            }],
            problems: vec![],
        };
        assert!(check(&way).unwrap_err().contains("mute"));
    }
}
