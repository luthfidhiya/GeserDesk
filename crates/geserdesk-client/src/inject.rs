// SPDX-License-Identifier: GPL-3.0-or-later
//! Turning protocol input events into synthesized OS input.
//!
//! [`InputSink`] is the seam. The session loop calls it; what's behind it is
//! chosen at startup:
//!
//! * [`NullSink`] -- discards everything (headless, `--dry-run`).
//! * [`RecordingSink`] -- keeps events for assertions (tests).
//! * [`EnigoSink`] -- real injection via `enigo` (feature `inject`, milestone
//!   M3). `enigo` wraps XTest on Linux and `SendInput` on Windows -- the same
//!   primitives Input Leap's `XWindowsScreen` / `MSWindowsScreen` use.

#[cfg(any(feature = "inject", test))]
use geserdesk_proto::KeyAction;
use geserdesk_proto::{KeyEvent, MouseEvent, Point};

/// Receives input events destined for this machine.
pub trait InputSink: Send {
    /// The cursor entered this screen; position it at `at` (absolute).
    fn enter(&mut self, at: Point);
    /// The cursor left this screen; release everything still held.
    fn leave(&mut self);
    fn key(&mut self, ev: &KeyEvent);
    fn mouse(&mut self, ev: MouseEvent);
}

/// A borrowed sink is a sink, so one sink can serve many connections in turn.
impl<T: InputSink + ?Sized> InputSink for &mut T {
    fn enter(&mut self, at: Point) {
        (**self).enter(at);
    }
    fn leave(&mut self) {
        (**self).leave();
    }
    fn key(&mut self, ev: &KeyEvent) {
        (**self).key(ev);
    }
    fn mouse(&mut self, ev: MouseEvent) {
        (**self).mouse(ev);
    }
}

/// A boxed sink is a sink, so the kind can be chosen at runtime.
impl<T: InputSink + ?Sized> InputSink for Box<T> {
    fn enter(&mut self, at: Point) {
        (**self).enter(at);
    }
    fn leave(&mut self) {
        (**self).leave();
    }
    fn key(&mut self, ev: &KeyEvent) {
        (**self).key(ev);
    }
    fn mouse(&mut self, ev: MouseEvent) {
        (**self).mouse(ev);
    }
}

/// Discards all input. Useful for `--dry-run` and headless runs.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullSink;

impl InputSink for NullSink {
    fn enter(&mut self, at: Point) {
        tracing::debug!(?at, "enter (null sink)");
    }
    fn leave(&mut self) {
        tracing::debug!("leave (null sink)");
    }
    fn key(&mut self, ev: &KeyEvent) {
        tracing::debug!(?ev, "key (null sink)");
    }
    fn mouse(&mut self, ev: MouseEvent) {
        tracing::debug!(?ev, "mouse (null sink)");
    }
}

/// Records every event for test assertions.
#[derive(Debug, Default)]
pub struct RecordingSink {
    pub enters: Vec<Point>,
    pub leaves: usize,
    pub keys: Vec<KeyEvent>,
    pub mice: Vec<MouseEvent>,
}

impl InputSink for RecordingSink {
    fn enter(&mut self, at: Point) {
        self.enters.push(at);
    }
    fn leave(&mut self) {
        self.leaves += 1;
    }
    fn key(&mut self, ev: &KeyEvent) {
        self.keys.push(ev.clone());
    }
    fn mouse(&mut self, ev: MouseEvent) {
        self.mice.push(ev);
    }
}

#[cfg(feature = "inject")]
pub use enigo_sink::EnigoSink;

#[cfg(feature = "inject")]
mod enigo_sink {
    use super::*;
    use enigo::{
        Axis, Button as EBtn, Coordinate, Direction, Enigo, Key, Keyboard, Mouse, Settings,
    };
    use geserdesk_proto::Button;
    use std::collections::{HashMap, HashSet};

    /// Real OS input injection.
    ///
    /// Remembers the exact [`Key`] it pressed for every physical key (by HID
    /// code), so a release -- including the forced release on
    /// [`leave`](InputSink::leave) -- always lifts the same key that went down,
    /// even if the character it produced has since changed.
    pub struct EnigoSink {
        enigo: Enigo,
        held_keys: HashMap<u16, Key>,
        held_buttons: HashSet<u8>,
    }

    impl EnigoSink {
        pub fn new() -> anyhow::Result<Self> {
            let enigo = Enigo::new(&Settings::default())
                .map_err(|e| anyhow::anyhow!("initialising enigo: {e}"))?;
            Ok(Self {
                enigo,
                held_keys: HashMap::new(),
                held_buttons: HashSet::new(),
            })
        }

        /// Size of the main display, for the `Hello` handshake.
        pub fn display_size(&self) -> anyhow::Result<(i32, i32)> {
            self.enigo
                .main_display()
                .map_err(|e| anyhow::anyhow!("reading display size: {e}"))
        }

        /// Move to an absolute position in desktop coordinates. On Windows
        /// this spans every monitor (enigo only maps the primary one, so a
        /// monitor above or left of it was unreachable).
        fn move_to(&mut self, x: i32, y: i32) -> Result<(), enigo::InputError> {
            #[cfg(windows)]
            if win::move_virtual(x, y) {
                return Ok(());
            }
            self.enigo.move_mouse(x, y, Coordinate::Abs)
        }

        fn move_by(&mut self, dx: i32, dy: i32) -> Result<(), enigo::InputError> {
            #[cfg(windows)]
            if let Some((x, y)) = win::cursor_pos() {
                return self.move_to(x + dx, y + dy);
            }
            self.enigo.move_mouse(dx, dy, Coordinate::Rel)
        }

        fn inject(&mut self, key: Key, dir: Direction) {
            if let Err(e) = self.enigo.key(key, dir) {
                tracing::warn!(error = %e, ?key, "key injection failed");
            }
        }
    }

    impl InputSink for EnigoSink {
        fn enter(&mut self, at: Point) {
            if let Err(e) = self.move_to(at.x, at.y) {
                tracing::warn!(error = %e, "move_mouse on enter failed");
            }
        }

        fn leave(&mut self) {
            // Release in reverse order of insertion is not tracked; modifiers
            // and normal keys are independent for the OS, so any order works.
            let keys: Vec<Key> = self.held_keys.drain().map(|(_, k)| k).collect();
            for key in keys {
                self.inject(key, Direction::Release);
            }
            let buttons: Vec<u8> = self.held_buttons.drain().collect();
            for b in buttons {
                let _ = self
                    .enigo
                    .button(map_button(decode_button(b)), Direction::Release);
            }
        }

        fn key(&mut self, ev: &KeyEvent) {
            match ev.action {
                KeyAction::Down | KeyAction::Repeat(_) => {
                    let Some(key) = key_for(ev) else {
                        tracing::warn!(hid = ev.hid, "no way to type this key; ignored");
                        return;
                    };
                    // A repeat re-presses the key that is already down.
                    let key = *self.held_keys.entry(ev.hid).or_insert(key);
                    self.inject(key, Direction::Press);
                }
                KeyAction::Up => {
                    // Release what we pressed; a key we never saw go down
                    // (e.g. the hotkey's own modifiers) is simply ignored.
                    if let Some(key) = self.held_keys.remove(&ev.hid) {
                        self.inject(key, Direction::Release);
                    }
                }
            }
        }

        fn mouse(&mut self, ev: MouseEvent) {
            let r = match ev {
                MouseEvent::MoveAbs { x, y } => self.move_to(x, y),
                MouseEvent::MoveRel { dx, dy } => self.move_by(dx, dy),
                MouseEvent::Button { button, down } => {
                    let code = encode_button(button);
                    if down {
                        self.held_buttons.insert(code);
                    } else {
                        self.held_buttons.remove(&code);
                    }
                    self.enigo.button(
                        map_button(button),
                        if down {
                            Direction::Press
                        } else {
                            Direction::Release
                        },
                    )
                }
                MouseEvent::Wheel { dx, dy } => {
                    let mut r = Ok(());
                    if dx != 0 {
                        r = self.enigo.scroll(notches(dx), Axis::Horizontal);
                    }
                    if dy != 0 && r.is_ok() {
                        r = self.enigo.scroll(-notches(dy), Axis::Vertical);
                    }
                    r
                }
            };
            if let Err(e) = r {
                tracing::warn!(error = %e, "mouse injection failed");
            }
        }
    }

    #[cfg(windows)]
    mod win {
        use windows_sys::Win32::Foundation::POINT;
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
            SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_MOVE,
            MOUSEEVENTF_VIRTUALDESK, MOUSEINPUT,
        };
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GetCursorPos, GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
            SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
        };

        pub fn cursor_pos() -> Option<(i32, i32)> {
            let mut p = POINT { x: 0, y: 0 };
            (unsafe { GetCursorPos(&mut p) } != 0).then_some((p.x, p.y))
        }

        fn send(dx: i32, dy: i32, flags: u32) -> bool {
            let input = INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 {
                    mi: MOUSEINPUT {
                        dx,
                        dy,
                        mouseData: 0,
                        dwFlags: MOUSEEVENTF_MOVE | flags,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            };
            unsafe { SendInput(1, &input, std::mem::size_of::<INPUT>() as i32) == 1 }
        }

        /// Absolute move over the whole virtual desktop, clamped to it.
        /// Returns false if the desktop size is unavailable or the injection
        /// failed.
        pub fn move_virtual(x: i32, y: i32) -> bool {
            let (vx, vy, w, h) = unsafe {
                (
                    GetSystemMetrics(SM_XVIRTUALSCREEN),
                    GetSystemMetrics(SM_YVIRTUALSCREEN),
                    GetSystemMetrics(SM_CXVIRTUALSCREEN),
                    GetSystemMetrics(SM_CYVIRTUALSCREEN),
                )
            };
            if w < 2 || h < 2 {
                return false;
            }
            let (cx, cy) = ((x - vx).clamp(0, w - 1), (y - vy).clamp(0, h - 1));
            let abs = MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK;
            // Microsoft's mapping: ceil(px * 65536 / size).
            let norm =
                |p: i32, size: i32| ((p as i64 * 65536 + size as i64 - 1) / size as i64) as i32;
            if cx == w - 1 {
                // Windows silently drops an absolute move onto the last pixel
                // column, so the cursor would stop one short of the right
                // edge. Go next to it, then push into the edge: relative
                // moves are clamped there.
                return send(norm(cx - 1, w), norm(cy, h), abs) && send(8, 0, 0);
            }
            send(norm(cx, w), norm(cy, h), abs)
        }
    }

    impl Drop for EnigoSink {
        fn drop(&mut self) {
            self.leave();
        }
    }

    /// Wheel deltas are 120 per notch; round away from zero so a small delta
    /// (a smooth-scrolling touchpad) still scrolls by one.
    fn notches(delta: i32) -> i32 {
        match delta {
            0 => 0,
            d if d.abs() < 120 => d.signum(),
            d => d / 120,
        }
    }

    /// Choose how to type a key: named keys by HID code, printable keys by the
    /// character the server's layout produced.
    pub(super) fn key_for(ev: &KeyEvent) -> Option<Key> {
        named_key(ev.hid).or_else(|| ev.ch.map(Key::Unicode))
    }

    /// Non-printable keys, identified purely by HID usage so they work under any
    /// keyboard layout.
    pub(super) fn named_key(hid: u16) -> Option<Key> {
        Some(match hid {
            0x28 | 0x58 => Key::Return,
            0x29 => Key::Escape,
            0x2A => Key::Backspace,
            0x2B => Key::Tab,
            0x2C => Key::Space,
            0x39 => Key::CapsLock,
            0x3A..=0x45 => [
                Key::F1,
                Key::F2,
                Key::F3,
                Key::F4,
                Key::F5,
                Key::F6,
                Key::F7,
                Key::F8,
                Key::F9,
                Key::F10,
                Key::F11,
                Key::F12,
            ][usize::from(hid - 0x3A)],
            0x46 => Key::PrintScr,
            0x48 => Key::Pause,
            0x49 => Key::Insert,
            0x4A => Key::Home,
            0x4B => Key::PageUp,
            0x4C => Key::Delete,
            0x4D => Key::End,
            0x4E => Key::PageDown,
            0x4F => Key::RightArrow,
            0x50 => Key::LeftArrow,
            0x51 => Key::DownArrow,
            0x52 => Key::UpArrow,
            0x53 => Key::Numlock,
            0x68..=0x6F => [
                Key::F13,
                Key::F14,
                Key::F15,
                Key::F16,
                Key::F17,
                Key::F18,
                Key::F19,
                Key::F20,
            ][usize::from(hid - 0x68)],
            0xE0 => Key::LControl,
            0xE1 => Key::LShift,
            0xE2 | 0xE6 => Key::Alt,
            0xE3 | 0xE7 => Key::Meta,
            0xE4 => Key::RControl,
            0xE5 => Key::RShift,
            _ => return None,
        })
    }

    fn map_button(b: Button) -> EBtn {
        match b {
            Button::Left => EBtn::Left,
            Button::Middle => EBtn::Middle,
            Button::Right => EBtn::Right,
            Button::Back => EBtn::Back,
            Button::Forward => EBtn::Forward,
            Button::Extra(_) => EBtn::Left,
        }
    }

    fn encode_button(b: Button) -> u8 {
        match b {
            Button::Left => 0,
            Button::Middle => 1,
            Button::Right => 2,
            Button::Back => 3,
            Button::Forward => 4,
            Button::Extra(n) => 10u8.saturating_add(n),
        }
    }

    fn decode_button(code: u8) -> Button {
        match code {
            0 => Button::Left,
            1 => Button::Middle,
            2 => Button::Right,
            3 => Button::Back,
            4 => Button::Forward,
            n => Button::Extra(n.saturating_sub(10)),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use geserdesk_proto::ModMask;

        fn ev(hid: u16, ch: Option<char>) -> KeyEvent {
            KeyEvent {
                hid,
                ch,
                mods: ModMask::empty(),
                action: KeyAction::Down,
            }
        }

        #[test]
        fn named_keys_ignore_the_character() {
            assert_eq!(key_for(&ev(0x28, None)), Some(Key::Return));
            assert_eq!(key_for(&ev(0x52, None)), Some(Key::UpArrow));
            assert_eq!(key_for(&ev(0x3A, None)), Some(Key::F1));
            assert_eq!(key_for(&ev(0x45, None)), Some(Key::F12));
            assert_eq!(key_for(&ev(0xE1, None)), Some(Key::LShift));
        }

        #[test]
        fn printable_keys_use_the_server_character() {
            assert_eq!(key_for(&ev(0x04, Some('a'))), Some(Key::Unicode('a')));
            assert_eq!(key_for(&ev(0x04, Some('A'))), Some(Key::Unicode('A')));
        }

        #[test]
        fn unknown_key_without_character_is_untypable() {
            assert_eq!(key_for(&ev(0x04, None)), None);
        }

        #[test]
        fn small_wheel_deltas_still_scroll() {
            assert_eq!(notches(0), 0);
            assert_eq!(notches(30), 1);
            assert_eq!(notches(-30), -1);
            assert_eq!(notches(240), 2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use geserdesk_proto::ModMask;

    #[test]
    fn recording_sink_captures_events() {
        let mut s = RecordingSink::default();
        s.enter(Point::new(1, 2));
        s.mouse(MouseEvent::Wheel { dx: 0, dy: 120 });
        s.key(&KeyEvent {
            hid: 4,
            ch: Some('a'),
            mods: ModMask::empty(),
            action: KeyAction::Down,
        });
        s.leave();
        assert_eq!(s.enters, vec![Point::new(1, 2)]);
        assert_eq!(s.mice.len(), 1);
        assert_eq!(s.keys.len(), 1);
        assert_eq!(s.leaves, 1);
    }

    #[test]
    fn null_sink_is_a_sink() {
        fn takes_sink(_: impl InputSink) {}
        takes_sink(NullSink);
    }
}
