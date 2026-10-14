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

/// Map a Windows key event to a HID keyboard usage, if it has one.
///
/// `scan` and `extended` are the hardware scan code and its `E0`-prefix flag as
/// a low-level keyboard hook reports them; `vk` is the virtual key. Scan codes
/// identify the physical key under any layout, so they decide, with three
/// exceptions where Windows reports them inconsistently and the virtual key is
/// the reliable witness: Num Lock (reported as extended), Pause (a multi-byte
/// sequence) and Print Screen. F13-F20 are only on some keyboards and their scan
/// codes vary, so they go by virtual key too.
pub fn win_key_to_hid(vk: u32, scan: u16, extended: bool) -> Option<u16> {
    match vk {
        0x90 => return Some(0x53),                             // VK_NUMLOCK
        0x13 => return Some(0x48),                             // VK_PAUSE
        0x2C => return Some(0x46),                             // VK_SNAPSHOT
        0x7C..=0x83 => return Some(0x68 + (vk - 0x7C) as u16), // F13..F20
        _ => {}
    }
    let hid = if extended {
        match scan {
            0x1C => 0x58, // KP Enter
            0x1D => 0xE4, // Right Ctrl
            0x35 => 0x54, // KP /
            0x38 => 0xE6, // Right Alt
            0x47 => 0x4A, // Home
            0x48 => 0x52, // Up
            0x49 => 0x4B, // Page Up
            0x4B => 0x50, // Left
            0x4D => 0x4F, // Right
            0x4F => 0x4D, // End
            0x50 => 0x51, // Down
            0x51 => 0x4E, // Page Down
            0x52 => 0x49, // Insert
            0x53 => 0x4C, // Delete
            0x5B => 0xE3, // Left Windows
            0x5C => 0xE7, // Right Windows
            0x5D => 0x65, // Menu
            _ => return None,
        }
    } else {
        match scan {
            0x01 => 0x29,                        // Escape
            0x02..=0x0A => 0x1E + (scan - 0x02), // 1..9
            0x0B => 0x27,                        // 0
            0x0C => 0x2D,
            0x0D => 0x2E,
            0x0E => 0x2A,                        // Backspace
            0x0F => 0x2B,                        // Tab
            0x10 => 0x14,                        // Q
            0x11 => 0x1A,                        // W
            0x12 => 0x08,                        // E
            0x13 => 0x15,                        // R
            0x14 => 0x17,                        // T
            0x15 => 0x1C,                        // Y
            0x16 => 0x18,                        // U
            0x17 => 0x0C,                        // I
            0x18 => 0x12,                        // O
            0x19 => 0x13,                        // P
            0x1A => 0x2F,                        // [
            0x1B => 0x30,                        // ]
            0x1C => 0x28,                        // Enter
            0x1D => 0xE0,                        // Left Ctrl
            0x1E => 0x04,                        // A
            0x1F => 0x16,                        // S
            0x20 => 0x07,                        // D
            0x21 => 0x09,                        // F
            0x22 => 0x0A,                        // G
            0x23 => 0x0B,                        // H
            0x24 => 0x0D,                        // J
            0x25 => 0x0E,                        // K
            0x26 => 0x0F,                        // L
            0x27 => 0x33,                        // ;
            0x28 => 0x34,                        // '
            0x29 => 0x35,                        // `
            0x2A => 0xE1,                        // Left Shift
            0x2B => 0x31,                        // backslash
            0x2C => 0x1D,                        // Z
            0x2D => 0x1B,                        // X
            0x2E => 0x06,                        // C
            0x2F => 0x19,                        // V
            0x30 => 0x05,                        // B
            0x31 => 0x11,                        // N
            0x32 => 0x10,                        // M
            0x33 => 0x36,                        // ,
            0x34 => 0x37,                        // .
            0x35 => 0x38,                        // /
            0x36 => 0xE5,                        // Right Shift
            0x37 => 0x55,                        // KP *
            0x38 => 0xE2,                        // Left Alt
            0x39 => 0x2C,                        // Space
            0x3A => 0x39,                        // Caps Lock
            0x3B..=0x44 => 0x3A + (scan - 0x3B), // F1..F10
            0x46 => 0x47,                        // Scroll Lock
            0x47 => 0x5F,                        // KP 7
            0x48 => 0x60,
            0x49 => 0x61,
            0x4A => 0x56, // KP -
            0x4B => 0x5C, // KP 4
            0x4C => 0x5D,
            0x4D => 0x5E,
            0x4E => 0x57, // KP +
            0x4F => 0x59, // KP 1
            0x50 => 0x5A,
            0x51 => 0x5B,
            0x52 => 0x62, // KP 0
            0x53 => 0x63, // KP .
            0x56 => 0x64, // non-US backslash
            0x57 => 0x44, // F11
            0x58 => 0x45, // F12
            _ => return None,
        }
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
    fn windows_scan_codes() {
        assert_eq!(win_key_to_hid(0x41, 0x1E, false), Some(0x04)); // A
        assert_eq!(win_key_to_hid(0x31, 0x02, false), Some(0x1E)); // 1
        assert_eq!(win_key_to_hid(0x30, 0x0B, false), Some(0x27)); // 0
        assert_eq!(win_key_to_hid(0x70, 0x3B, false), Some(0x3A)); // F1
        assert_eq!(win_key_to_hid(0x7B, 0x58, false), Some(0x45)); // F12
        assert_eq!(win_key_to_hid(0xA2, 0x1D, false), Some(0xE0)); // Left Ctrl
        assert_eq!(win_key_to_hid(0xA3, 0x1D, true), Some(0xE4)); // Right Ctrl
        assert_eq!(win_key_to_hid(0xA5, 0x38, true), Some(0xE6)); // Right Alt
    }

    #[test]
    fn windows_tells_the_keypad_from_the_navigation_cluster() {
        // The same scan code: extended is Home, plain is keypad 7.
        assert_eq!(win_key_to_hid(0x24, 0x47, true), Some(0x4A));
        assert_eq!(win_key_to_hid(0x67, 0x47, false), Some(0x5F));
        assert_eq!(win_key_to_hid(0x0D, 0x1C, true), Some(0x58)); // KP Enter
        assert_eq!(win_key_to_hid(0x0D, 0x1C, false), Some(0x28)); // Enter
    }

    #[test]
    fn windows_keys_that_go_by_virtual_key() {
        assert_eq!(win_key_to_hid(0x90, 0x45, true), Some(0x53)); // Num Lock
        assert_eq!(win_key_to_hid(0x13, 0x45, false), Some(0x48)); // Pause
        assert_eq!(win_key_to_hid(0x2C, 0x54, false), Some(0x46)); // Print Screen
        assert_eq!(win_key_to_hid(0x7C, 0x64, false), Some(0x68)); // F13
        assert_eq!(win_key_to_hid(0x83, 0x6B, false), Some(0x6F)); // F20
    }

    #[test]
    fn windows_unknown_keys_are_none() {
        assert_eq!(win_key_to_hid(0xFF, 0x00, false), None);
        assert_eq!(win_key_to_hid(0xFF, 0x7F, true), None);
    }

    /// The Windows table and the evdev one name the same physical keys: every
    /// HID usage the Linux side can send has a Windows key that produces it, so
    /// a Windows server can drive whatever a Linux one can.
    #[test]
    fn windows_covers_every_key_linux_does() {
        let win: std::collections::HashSet<u16> = (0..=0xFFu32)
            .flat_map(|vk| {
                (0..=0x7Fu16).flat_map(move |scan| {
                    [false, true]
                        .into_iter()
                        .filter_map(move |ext| win_key_to_hid(vk, scan, ext))
                })
            })
            .collect();
        for code in 0..300u16 {
            if let Some(hid) = evdev_to_hid(code) {
                assert!(win.contains(&hid), "no Windows key gives HID {hid:#x}");
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
