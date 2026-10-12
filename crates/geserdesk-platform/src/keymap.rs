// SPDX-License-Identifier: GPL-3.0-or-later
//! Physical key identity: evdev scancodes to USB HID usage codes.
//!
//! X11 keycodes are `evdev + 8` on every modern Linux X server, so the capture
//! backend subtracts 8 and looks the result up here. The wire protocol carries
//! only the HID usage (page 0x07), which is stable across layouts.

/// Map a Linux evdev key code to a HID keyboard usage, if it has one.
pub fn evdev_to_hid(code: u16) -> Option<u16> {
    let hid = match code {
        // Letters (evdev follows the QWERTY rows, HID is alphabetical).
        30 => 0x04, // A
        48 => 0x05, // B
        46 => 0x06, // C
        32 => 0x07, // D
        18 => 0x08, // E
        33 => 0x09, // F
        34 => 0x0A, // G
        35 => 0x0B, // H
        23 => 0x0C, // I
        36 => 0x0D, // J
        37 => 0x0E, // K
        38 => 0x0F, // L
        50 => 0x10, // M
        49 => 0x11, // N
        24 => 0x12, // O
        25 => 0x13, // P
        16 => 0x14, // Q
        19 => 0x15, // R
        31 => 0x16, // S
        20 => 0x17, // T
        22 => 0x18, // U
        47 => 0x19, // V
        17 => 0x1A, // W
        45 => 0x1B, // X
        21 => 0x1C, // Y
        44 => 0x1D, // Z
        // Digits 1..9, 0.
        2..=10 => 0x1E + (code - 2),
        11 => 0x27,
        28 => 0x28, // Enter
        1 => 0x29,  // Escape
        14 => 0x2A, // Backspace
        15 => 0x2B, // Tab
        57 => 0x2C, // Space
        12 => 0x2D, // -
        13 => 0x2E, // =
        26 => 0x2F, // [
        27 => 0x30, // ]
        43 => 0x31, // backslash
        39 => 0x33, // ;
        40 => 0x34, // '
        41 => 0x35, // `
        51 => 0x36, // ,
        52 => 0x37, // .
        53 => 0x38, // /
        58 => 0x39, // Caps Lock
        // F1..F10, F11, F12
        59..=68 => 0x3A + (code - 59),
        87 => 0x44,
        88 => 0x45,
        99 => 0x46,  // Print Screen / SysRq
        70 => 0x47,  // Scroll Lock
        119 => 0x48, // Pause
        110 => 0x49, // Insert
        102 => 0x4A, // Home
        104 => 0x4B, // Page Up
        111 => 0x4C, // Delete
        107 => 0x4D, // End
        109 => 0x4E, // Page Down
        106 => 0x4F, // Right
        105 => 0x50, // Left
        108 => 0x51, // Down
        103 => 0x52, // Up
        69 => 0x53,  // Num Lock
        98 => 0x54,  // KP /
        55 => 0x55,  // KP *
        74 => 0x56,  // KP -
        78 => 0x57,  // KP +
        96 => 0x58,  // KP Enter
        79 => 0x59,  // KP 1
        80 => 0x5A,
        81 => 0x5B,
        75 => 0x5C, // KP 4
        76 => 0x5D,
        77 => 0x5E,
        71 => 0x5F, // KP 7
        72 => 0x60,
        73 => 0x61,
        82 => 0x62,  // KP 0
        83 => 0x63,  // KP .
        86 => 0x64,  // non-US backslash (102nd key)
        127 => 0x65, // Menu / Compose
        // Modifiers (HID 0xE0..0xE7).
        29 => 0xE0,  // Left Ctrl
        42 => 0xE1,  // Left Shift
        56 => 0xE2,  // Left Alt
        125 => 0xE3, // Left Meta
        97 => 0xE4,  // Right Ctrl
        54 => 0xE5,  // Right Shift
        100 => 0xE6, // Right Alt
        126 => 0xE7, // Right Meta
        _ => return None,
    };
    Some(hid)
}

/// Whether the HID usage is a printable character key (letters, digits,
/// punctuation, keypad digits and operators). Those travel with a `ch`; every
/// other key is identified purely by its HID usage.
pub fn hid_is_printable(hid: u16) -> bool {
    matches!(hid, 0x04..=0x27 | 0x2D..=0x38 | 0x54..=0x57 | 0x59..=0x63 | 0x64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_and_digits() {
        assert_eq!(evdev_to_hid(30), Some(0x04)); // A
        assert_eq!(evdev_to_hid(44), Some(0x1D)); // Z
        assert_eq!(evdev_to_hid(2), Some(0x1E)); // 1
        assert_eq!(evdev_to_hid(10), Some(0x26)); // 9
        assert_eq!(evdev_to_hid(11), Some(0x27)); // 0
    }

    #[test]
    fn function_navigation_and_modifiers() {
        assert_eq!(evdev_to_hid(59), Some(0x3A)); // F1
        assert_eq!(evdev_to_hid(68), Some(0x43)); // F10
        assert_eq!(evdev_to_hid(88), Some(0x45)); // F12
        assert_eq!(evdev_to_hid(103), Some(0x52)); // Up
        assert_eq!(evdev_to_hid(29), Some(0xE0)); // Left Ctrl
        assert_eq!(evdev_to_hid(126), Some(0xE7)); // Right Meta
    }

    #[test]
    fn unknown_codes_are_none() {
        assert_eq!(evdev_to_hid(0), None);
        assert_eq!(evdev_to_hid(500), None);
    }

    #[test]
    fn every_mapped_hid_is_unique() {
        let mut seen = std::collections::HashSet::new();
        for code in 0..300u16 {
            if let Some(h) = evdev_to_hid(code) {
                assert!(seen.insert(h), "duplicate HID {h:#x} for evdev {code}");
            }
        }
    }

    #[test]
    fn printable_classification() {
        assert!(hid_is_printable(0x04)); // a
        assert!(hid_is_printable(0x2D)); // -
        assert!(!hid_is_printable(0x28)); // Enter
        assert!(!hid_is_printable(0x2C)); // Space (named key on the client)
        assert!(!hid_is_printable(0x3A)); // F1
        assert!(!hid_is_printable(0xE1)); // Shift
    }
}
