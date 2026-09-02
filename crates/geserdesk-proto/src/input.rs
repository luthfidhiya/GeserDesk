// SPDX-License-Identifier: GPL-3.0-or-later
//! Key and mouse event payloads.
//!
//! The key model follows the reasoning behind Input Leap's `KeyID` +
//! `KeyButton` split (see Input Leap `key_types.h`): on release the produced
//! character may differ from the one on press (dead keys, mismatched layouts),
//! so the receiver needs a stable *physical* identifier to know which key to
//! release. Input Leap used a private-use codepoint scheme; we use the HID
//! usage code, which has off-the-shelf mapping tables to evdev / Windows VK /
//! macOS on every platform.

use serde::{Deserialize, Serialize};

/// A minimal `bitflags`-style wrapper so the crate carries no external bitflags
/// dependency. Defined before first use (macro_rules is order-sensitive).
macro_rules! bitflags_like {
    (
        $(#[$meta:meta])*
        pub struct $name:ident : $ty:ty {
            $( const $flag:ident = $value:expr; )*
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
        pub struct $name { bits: $ty }

        impl $name {
            $( pub const $flag: $name = $name { bits: $value }; )*

            pub const fn empty() -> Self { $name { bits: 0 } }
            pub const fn bits(self) -> $ty { self.bits }
            pub const fn from_bits_truncate(bits: $ty) -> Self {
                let mut mask: $ty = 0;
                $( mask |= $value; )*
                $name { bits: bits & mask }
            }
            pub const fn contains(self, other: Self) -> bool {
                (self.bits & other.bits) == other.bits
            }
            pub fn insert(&mut self, other: Self) { self.bits |= other.bits; }
            pub fn remove(&mut self, other: Self) { self.bits &= !other.bits; }
            pub fn set(&mut self, other: Self, on: bool) {
                if on { self.insert(other) } else { self.remove(other) }
            }
            pub fn is_empty(self) -> bool { self.bits == 0 }
        }

        impl core::ops::BitOr for $name {
            type Output = Self;
            fn bitor(self, rhs: Self) -> Self { $name { bits: self.bits | rhs.bits } }
        }
        impl core::ops::BitAnd for $name {
            type Output = Self;
            fn bitand(self, rhs: Self) -> Self { $name { bits: self.bits & rhs.bits } }
        }
        impl core::ops::BitOrAssign for $name {
            fn bitor_assign(&mut self, rhs: Self) { self.bits |= rhs.bits; }
        }
    };
}

bitflags_like! {
    /// Active modifier keys, as a bitmask.
    pub struct ModMask: u32 {
        const SHIFT     = 0x0001;
        const CONTROL   = 0x0002;
        const ALT       = 0x0004;
        const META      = 0x0008;
        const SUPER     = 0x0010;
        const ALT_GR    = 0x0020;
        const CAPS_LOCK = 0x1000;
        const NUM_LOCK  = 0x2000;
    }
}

/// What happened to a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeyAction {
    Down,
    Up,
    /// Auto-repeat, carrying the repeat count since the last distinct event.
    Repeat(u16),
}

/// A single key event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyEvent {
    /// HID usage code (usage page 0x07 for the keyboard). Stable physical id.
    pub hid: u16,
    /// The character the server's layout produced, if any. Used as a fallback
    /// when the client's layout can't reach the same symbol from `hid`.
    pub ch: Option<char>,
    /// Modifier state at the time of the event.
    pub mods: ModMask,
    pub action: KeyAction,
}

/// Mouse buttons. `Extra` carries a 1-based index for buttons beyond the named
/// five (some mice have many).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Button {
    Left,
    Middle,
    Right,
    Back,
    Forward,
    Extra(u8),
}

/// A single mouse event. Absolute moves position the cursor on entry; relative
/// moves carry the rest of the session so the client's pointer acceleration is
/// bypassed and the cursor can't drift off the far edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MouseEvent {
    MoveAbs {
        x: i32,
        y: i32,
    },
    MoveRel {
        dx: i32,
        dy: i32,
    },
    Button {
        button: Button,
        down: bool,
    },
    /// Wheel deltas in the same 120-per-notch unit Input Leap and Windows use.
    Wheel {
        dx: i32,
        dy: i32,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modmask_ops() {
        let m = ModMask::SHIFT | ModMask::CONTROL;
        assert!(m.contains(ModMask::SHIFT));
        assert!(m.contains(ModMask::CONTROL));
        assert!(!m.contains(ModMask::ALT));

        let mut m2 = m;
        m2.remove(ModMask::SHIFT);
        assert!(!m2.contains(ModMask::SHIFT));
        assert!(m2.contains(ModMask::CONTROL));

        m2.set(ModMask::ALT, true);
        assert!(m2.contains(ModMask::ALT));
    }

    #[test]
    fn modmask_from_bits_truncate_drops_unknown() {
        let all = ModMask::SHIFT
            | ModMask::CONTROL
            | ModMask::ALT
            | ModMask::META
            | ModMask::SUPER
            | ModMask::ALT_GR
            | ModMask::CAPS_LOCK
            | ModMask::NUM_LOCK;
        assert_eq!(ModMask::from_bits_truncate(0xFFFF_FFFF).bits(), all.bits());
    }

    #[test]
    fn key_event_is_constructible() {
        let e = KeyEvent {
            hid: 0x04, // 'a'
            ch: Some('a'),
            mods: ModMask::empty(),
            action: KeyAction::Down,
        };
        assert_eq!(e.action, KeyAction::Down);
    }
}
