// SPDX-License-Identifier: GPL-3.0-or-later
//! What a machine's own `keys.toml` can say about special keys, as plain data and the
//! parsing of its words (the file itself is read by the app).
//!
//! Two things belong to the machine, whichever part it plays:
//! * a **rule** says that a key combination on this machine's keyboard means an action
//!   (`ctrl+alt+f5` is `brightness_down`), for the keys a laptop's Fn row does not
//!   show to programs;
//! * a **perform** override says how this machine does an action it is sent (press
//!   another key, or nothing).
//!
//! The two meet only in the name of the action, a list the code owns
//! ([`consumer::ACTIONS`]), so machines never need to agree on anything else.

use crate::consumer;
use crate::input::ModMask;

/// A key combination that stands for an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rule {
    /// The modifiers that must be held (no more, no fewer; the lock keys do not count).
    pub mods: ModMask,
    /// The key pressed with them (a keyboard usage).
    pub hid: u16,
    /// The action it means (an action code of [`consumer`]).
    pub action: u16,
}

/// The modifiers that count in a rule: the lock keys are left out.
pub fn rule_mods(mods: ModMask) -> ModMask {
    mods & (ModMask::SHIFT | ModMask::CONTROL | ModMask::ALT | ModMask::SUPER | ModMask::ALT_GR)
}

impl Rule {
    /// `"ctrl+alt+f5"` and the action's name.
    pub fn parse(trigger: &str, action: &str) -> Result<Rule, String> {
        let action_hid = consumer::from_name(action.trim())
            .ok_or_else(|| format!("unknown action {action:?}"))?;
        let mut mods = ModMask::empty();
        let mut key = None;
        for part in trigger.split('+').map(|p| p.trim().to_ascii_lowercase()) {
            let bit = match part.as_str() {
                "ctrl" | "control" => Some(ModMask::CONTROL),
                "alt" => Some(ModMask::ALT),
                "altgr" => Some(ModMask::ALT_GR),
                "shift" => Some(ModMask::SHIFT),
                "win" | "super" | "meta" | "cmd" => Some(ModMask::SUPER),
                _ => None,
            };
            match bit {
                Some(bit) => mods |= bit,
                None if key.is_none() => {
                    key = Some(key_hid(&part).ok_or_else(|| format!("unknown key {part:?}"))?)
                }
                None => return Err(format!("{trigger:?} has two keys")),
            }
        }
        let hid = key.ok_or_else(|| format!("{trigger:?} has no key, only modifiers"))?;
        Ok(Rule {
            mods,
            hid,
            action: action_hid,
        })
    }

    /// Whether a key press with these modifiers is this rule's combination.
    pub fn matches(&self, hid: u16, mods: ModMask) -> bool {
        self.hid == hid && rule_mods(mods) == self.mods
    }
}

/// The keyboard usage of a key called `name` (lower case): letters, digits, F1 to F24, the
/// navigation keys, or `hid:0x..` for any usage.
pub fn key_hid(name: &str) -> Option<u16> {
    if let Some(raw) = name.strip_prefix("hid:") {
        return number(raw)
            .and_then(|n| u16::try_from(n).ok())
            .filter(|h| *h < 0x100);
    }
    let named = match name {
        "enter" | "return" => 0x28,
        "esc" | "escape" => 0x29,
        "backspace" => 0x2A,
        "tab" => 0x2B,
        "space" => 0x2C,
        "printscreen" | "prtsc" => 0x46,
        "scrolllock" => 0x47,
        "pause" => 0x48,
        "insert" | "ins" => 0x49,
        "home" => 0x4A,
        "pageup" | "pgup" => 0x4B,
        "delete" | "del" => 0x4C,
        "end" => 0x4D,
        "pagedown" | "pgdn" => 0x4E,
        "right" => 0x4F,
        "left" => 0x50,
        "down" => 0x51,
        "up" => 0x52,
        _ => 0,
    };
    if named != 0 {
        return Some(named);
    }
    let mut chars = name.chars();
    match (chars.next(), chars.next()) {
        (Some(c @ 'a'..='z'), None) => Some(0x04 + (c as u16 - 'a' as u16)),
        (Some(c @ '1'..='9'), None) => Some(0x1E + (c as u16 - '1' as u16)),
        (Some('0'), None) => Some(0x27),
        (Some('f'), Some(_)) => match name[1..].parse::<u16>().ok()? {
            n @ 1..=12 => Some(0x3A + (n - 1)),
            n @ 13..=24 => Some(0x68 + (n - 13)),
            _ => None,
        },
        _ => None,
    }
}

/// How a machine does an action it is sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Perform {
    /// Nothing at all.
    Ignore,
    /// Press the key with this X11 keysym (a Linux machine).
    Keysym(u32),
    /// Press the key with this Windows virtual-key code.
    VirtualKey(u32),
    /// Do another action instead.
    Action(u16),
}

impl Perform {
    /// `"ignore"`, `"keysym:0x1008ff02"`, `"vk:0xB7"` or `"action:volume_up"`. To keep the
    /// built-in way, leave the action out.
    pub fn parse(text: &str) -> Result<Perform, String> {
        let text = text.trim();
        if text.eq_ignore_ascii_case("ignore") {
            return Ok(Perform::Ignore);
        }
        let (kind, rest) = text.split_once(':').ok_or_else(|| {
            format!("{text:?}: use ignore, keysym:0x..., vk:0x... or action:name")
        })?;
        let rest = rest.trim();
        match kind.trim().to_ascii_lowercase().as_str() {
            "keysym" => number(rest)
                .map(Perform::Keysym)
                .ok_or_else(|| format!("{rest:?} is not a number")),
            "vk" => number(rest)
                .filter(|v| (1..=0xFE).contains(v))
                .map(Perform::VirtualKey)
                .ok_or_else(|| format!("{rest:?} is not a virtual key (1 to 0xFE)")),
            "action" => consumer::from_name(rest)
                .map(Perform::Action)
                .ok_or_else(|| format!("unknown action {rest:?}")),
            other => Err(format!("unknown way {other:?}")),
        }
    }
}

/// A decimal or `0x` number.
fn number(text: &str) -> Option<u32> {
    let text = text.trim();
    match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => u32::from_str_radix(&hex.replace('_', ""), 16).ok(),
        None => text.parse().ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rule_reads_modifiers_a_key_and_an_action() {
        let r = Rule::parse("Ctrl+Alt+F5", "brightness_down").unwrap();
        assert_eq!(r.mods, ModMask::CONTROL | ModMask::ALT);
        assert_eq!(r.hid, 0x3E); // F5
        assert_eq!(r.action, consumer::BRIGHTNESS_DOWN);
        assert_eq!(
            Rule::parse("win+shift+m", "mute").unwrap().mods,
            ModMask::SUPER | ModMask::SHIFT
        );
        assert_eq!(
            Rule::parse("f20", "mic_mute").unwrap().mods,
            ModMask::empty()
        );
    }

    #[test]
    fn a_rule_that_cannot_work_is_refused_with_why() {
        assert!(Rule::parse("ctrl+alt", "mute")
            .unwrap_err()
            .contains("only modifiers"));
        assert!(Rule::parse("ctrl+a+b", "mute")
            .unwrap_err()
            .contains("two keys"));
        assert!(Rule::parse("ctrl+banana", "mute")
            .unwrap_err()
            .contains("unknown key"));
        assert!(Rule::parse("ctrl+f5", "teleport")
            .unwrap_err()
            .contains("unknown action"));
    }

    #[test]
    fn a_rule_matches_its_modifiers_exactly_and_ignores_the_lock_keys() {
        let r = Rule::parse("ctrl+alt+f5", "brightness_down").unwrap();
        let both = ModMask::CONTROL | ModMask::ALT;
        assert!(r.matches(0x3E, both));
        assert!(r.matches(0x3E, both | ModMask::CAPS_LOCK | ModMask::NUM_LOCK));
        assert!(!r.matches(0x3E, ModMask::CONTROL));
        assert!(!r.matches(0x3E, both | ModMask::SHIFT));
        assert!(!r.matches(0x3F, both));
    }

    #[test]
    fn keys_are_found_by_name() {
        assert_eq!(key_hid("a"), Some(0x04));
        assert_eq!(key_hid("z"), Some(0x1D));
        assert_eq!(key_hid("1"), Some(0x1E));
        assert_eq!(key_hid("0"), Some(0x27));
        assert_eq!(key_hid("f1"), Some(0x3A));
        assert_eq!(key_hid("f12"), Some(0x45));
        assert_eq!(key_hid("f13"), Some(0x68));
        assert_eq!(key_hid("f24"), Some(0x73));
        assert_eq!(key_hid("f25"), None);
        assert_eq!(key_hid("up"), Some(0x52));
        assert_eq!(key_hid("hid:0x3e"), Some(0x3E));
        assert_eq!(key_hid("hid:0x1234"), None);
        assert_eq!(key_hid("fx"), None);
    }

    #[test]
    fn a_way_to_perform_an_action_is_parsed() {
        assert_eq!(Perform::parse("ignore"), Ok(Perform::Ignore));
        assert_eq!(
            Perform::parse(" KEYSYM:0x1008FF02 "),
            Ok(Perform::Keysym(0x1008_FF02))
        );
        assert_eq!(Perform::parse("vk:0xB7"), Ok(Perform::VirtualKey(0xB7)));
        assert_eq!(Perform::parse("vk:183"), Ok(Perform::VirtualKey(183)));
        assert_eq!(
            Perform::parse("action:volume_up"),
            Ok(Perform::Action(consumer::VOLUME_UP))
        );
        assert!(Perform::parse("vk:0x1FF").is_err());
        assert!(Perform::parse("action:nope").is_err());
        assert!(Perform::parse("default").is_err());
        assert!(Perform::parse("press x").is_err());
    }
}
