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
use serde::Deserialize;
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
}
