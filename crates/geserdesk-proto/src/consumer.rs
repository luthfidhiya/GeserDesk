// SPDX-License-Identifier: GPL-3.0-or-later
//! Keys that are not on the keyboard usage page (0x07) of the USB HID tables: volume and
//! media keys, browser and launcher keys, brightness and microphone mute, the ones on
//! the top row of a laptop.
//!
//! They travel in [`KeyEvent::hid`](crate::KeyEvent) as the page number in the high
//! byte and the usage below it (the Consumer page 0x0C, the Telephony page 0x0B), so
//! none can be mistaken for a keyboard usage, which are all under 0x100. A peer that
//! does not know one ignores it.

const CONSUMER: u16 = 0x0C00;
const TELEPHONY: u16 = 0x0B00;

/// Whether `hid` is one of these keys and not a keyboard usage.
pub const fn is_extra(hid: u16) -> bool {
    hid >= 0x0B00
}

// Sound.
pub const MUTE: u16 = CONSUMER + 0xE2;
pub const VOLUME_UP: u16 = CONSUMER + 0xE9;
pub const VOLUME_DOWN: u16 = CONSUMER + 0xEA;
/// Microphone mute (Phone Mute, on the Telephony page).
pub const MIC_MUTE: u16 = TELEPHONY + 0x2F;

// Media.
pub const PLAY_PAUSE: u16 = CONSUMER + 0xCD;
pub const NEXT_TRACK: u16 = CONSUMER + 0xB5;
pub const PREVIOUS_TRACK: u16 = CONSUMER + 0xB6;
pub const STOP: u16 = CONSUMER + 0xB7;

// The screen.
pub const BRIGHTNESS_UP: u16 = CONSUMER + 0x6F;
pub const BRIGHTNESS_DOWN: u16 = CONSUMER + 0x70;

// Launchers.
pub const MAIL: u16 = CONSUMER + 0x18A;
pub const CALCULATOR: u16 = CONSUMER + 0x192;
/// "My Computer" / the file explorer.
pub const EXPLORER: u16 = CONSUMER + 0x194;
pub const BROWSER: u16 = CONSUMER + 0x196;

// Browser keys.
pub const BROWSER_SEARCH: u16 = CONSUMER + 0x221;
pub const BROWSER_HOME: u16 = CONSUMER + 0x223;
pub const BROWSER_BACK: u16 = CONSUMER + 0x224;
pub const BROWSER_FORWARD: u16 = CONSUMER + 0x225;
pub const BROWSER_STOP: u16 = CONSUMER + 0x226;
pub const BROWSER_REFRESH: u16 = CONSUMER + 0x227;
pub const BROWSER_FAVORITES: u16 = CONSUMER + 0x22A;

/// The names of the actions, as `keys.toml` writes them.
pub const ACTIONS: &[(&str, u16)] = &[
    ("mute", MUTE),
    ("volume_up", VOLUME_UP),
    ("volume_down", VOLUME_DOWN),
    ("mic_mute", MIC_MUTE),
    ("play_pause", PLAY_PAUSE),
    ("next_track", NEXT_TRACK),
    ("previous_track", PREVIOUS_TRACK),
    ("stop", STOP),
    ("brightness_up", BRIGHTNESS_UP),
    ("brightness_down", BRIGHTNESS_DOWN),
    ("mail", MAIL),
    ("calculator", CALCULATOR),
    ("explorer", EXPLORER),
    ("browser", BROWSER),
    ("browser_search", BROWSER_SEARCH),
    ("browser_home", BROWSER_HOME),
    ("browser_back", BROWSER_BACK),
    ("browser_forward", BROWSER_FORWARD),
    ("browser_stop", BROWSER_STOP),
    ("browser_refresh", BROWSER_REFRESH),
    ("browser_favorites", BROWSER_FAVORITES),
];

/// The code of the action called `name`.
pub fn from_name(name: &str) -> Option<u16> {
    ACTIONS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, hid)| *hid)
}

/// What the action with this code is called.
pub fn name(hid: u16) -> Option<&'static str> {
    ACTIONS.iter().find(|(_, h)| *h == hid).map(|(n, _)| *n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn these_keys_are_told_from_keyboard_usages_and_from_each_other() {
        let all = [
            MUTE,
            VOLUME_UP,
            VOLUME_DOWN,
            MIC_MUTE,
            PLAY_PAUSE,
            NEXT_TRACK,
            PREVIOUS_TRACK,
            STOP,
            BRIGHTNESS_UP,
            BRIGHTNESS_DOWN,
            MAIL,
            CALCULATOR,
            EXPLORER,
            BROWSER,
            BROWSER_SEARCH,
            BROWSER_HOME,
            BROWSER_BACK,
            BROWSER_FORWARD,
            BROWSER_STOP,
            BROWSER_REFRESH,
            BROWSER_FAVORITES,
        ];
        let mut seen = std::collections::HashSet::new();
        for hid in all {
            assert!(is_extra(hid), "{hid:#x}");
            assert!(seen.insert(hid), "{hid:#x} twice");
        }
        // The keyboard page ends at 0xE7 (Right Meta).
        assert!(!is_extra(0xE7) && !is_extra(0x04));
    }

    #[test]
    fn every_action_has_one_name_and_back() {
        let mut names = std::collections::HashSet::new();
        for (n, hid) in ACTIONS {
            assert!(names.insert(*n), "{n} twice");
            assert!(is_extra(*hid), "{n}");
            assert_eq!(from_name(n), Some(*hid));
            assert_eq!(name(*hid), Some(*n));
        }
        assert_eq!(ACTIONS.len(), 21);
        assert_eq!(from_name("nope"), None);
        assert_eq!(name(0x04), None);
    }
}
