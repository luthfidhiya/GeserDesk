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

use geserdesk_proto::consumer;
use geserdesk_proto::keys::Perform;
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

/// How this machine does the key it was sent, once its own setup (`keys.toml`) is applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolved {
    /// The built-in way, for this key (the one sent, or another action's).
    Key(u16),
    /// Nothing.
    Ignore,
    /// This X11 keysym.
    Keysym(u32),
    /// This Windows virtual key.
    VirtualKey(u32),
}

/// What to do for `hid`. Only the special keys (actions) can be set up; every other key is
/// pressed the usual way. "Do another action" goes one step and no further, so two actions
/// that name each other cannot loop.
pub fn resolve(perform: &std::collections::HashMap<u16, Perform>, hid: u16) -> Resolved {
    if !consumer::is_extra(hid) {
        return Resolved::Key(hid);
    }
    match perform.get(&hid) {
        None => Resolved::Key(hid),
        Some(Perform::Ignore) => Resolved::Ignore,
        Some(Perform::Keysym(v)) => Resolved::Keysym(*v),
        Some(Perform::VirtualKey(v)) => Resolved::VirtualKey(*v),
        Some(Perform::Action(other)) => Resolved::Key(*other),
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
    use geserdesk_proto::consumer as c;
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
        /// This machine's own way of doing the special keys it is sent.
        perform: HashMap<u16, Perform>,
    }

    impl EnigoSink {
        pub fn new() -> anyhow::Result<Self> {
            let enigo = Enigo::new(&Settings::default())
                .map_err(|e| anyhow::anyhow!("initialising enigo: {e}"))?;
            Ok(Self {
                enigo,
                held_keys: HashMap::new(),
                held_buttons: HashSet::new(),
                perform: HashMap::new(),
            })
        }

        /// Do the special keys this machine's own way (see [`resolve`]).
        pub fn with_perform(mut self, perform: HashMap<u16, Perform>) -> Self {
            self.perform = perform;
            self
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
                    let key = match resolve(&self.perform, ev.hid) {
                        Resolved::Ignore => return,
                        Resolved::Key(hid) => key_for(&KeyEvent { hid, ..ev.clone() }),
                        #[cfg(all(unix, not(target_os = "macos")))]
                        Resolved::Keysym(v) => Some(Key::Other(v)),
                        #[cfg(windows)]
                        Resolved::VirtualKey(v) => Some(Key::Other(v)),
                        #[allow(unreachable_patterns)]
                        _ => {
                            tracing::warn!(
                                hid = ev.hid,
                                "keys.toml names a key of another system; ignored"
                            );
                            return;
                        }
                    };
                    let Some(key) = key else {
                        tracing::warn!(hid = ev.hid, "no way to type this key; ignored");
                        return;
                    };
                    let key = os_key(key);
                    // A character with no key on this layout is typed as text,
                    // which enigo sends as a whole press-and-release on each
                    // call: once on press, nothing to release.
                    #[cfg(windows)]
                    if let Key::Unicode(c) = key {
                        if win::text_only(c) {
                            if let Err(e) = self.enigo.key(key, Direction::Click) {
                                tracing::warn!(error = %e, ?key, "key injection failed");
                            }
                            return;
                        }
                    }
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

    /// Adjust a key to what this OS's injection can type.
    ///
    /// enigo 0.3 on Windows finds the virtual key of a character with
    /// `VkKeyScanExW` but keeps that call's shift-state byte as part of the key
    /// code (`'A'` becomes 0x0141, not 0x41), so every character that needs
    /// Shift -- capitals, `!`, `@`, `?`, ... -- turns into an invalid key and
    /// types nothing. Look the key up here and hand enigo the plain virtual
    /// key: the server sends Shift as a key of its own, so the OS already has
    /// it held.
    fn os_key(key: Key) -> Key {
        #[cfg(windows)]
        if let Key::Unicode(c) = key {
            if let Some(vk) = win::shifted_vk(c) {
                return Key::Other(u32::from(vk));
            }
        }
        key
    }

    #[cfg(windows)]
    mod win {
        use windows_sys::Win32::Foundation::POINT;
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
            GetKeyboardLayout, SendInput, VkKeyScanExW, INPUT, INPUT_0, INPUT_MOUSE,
            MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_MOVE, MOUSEEVENTF_VIRTUALDESK, MOUSEINPUT,
        };
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GetCursorPos, GetForegroundWindow, GetSystemMetrics, GetWindowThreadProcessId,
            SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
        };

        /// What the layout of the focused window says about typing `c`: the
        /// virtual key and the modifier bits (shift 1, ctrl 2, alt 4) it needs,
        /// or `None` if the character has no key on it.
        fn lookup(c: char) -> Option<(u16, i16)> {
            let mut buf = [0u16; 2];
            let [unit] = *c.encode_utf16(&mut buf) else {
                return None; // outside the BMP: always typed as text
            };
            let layout = unsafe {
                GetKeyboardLayout(GetWindowThreadProcessId(
                    GetForegroundWindow(),
                    std::ptr::null_mut(),
                ))
            };
            let found = unsafe { VkKeyScanExW(unit, layout) };
            (found != -1).then_some(((found & 0xff) as u16, (found >> 8) & 0xff))
        }

        /// The virtual key that types `c` with a modifier held, if the layout
        /// needs one; `None` for characters typed plainly (enigo handles those)
        /// or not on the layout at all.
        pub fn shifted_vk(c: char) -> Option<u16> {
            lookup(c).filter(|&(_, state)| state != 0).map(|(vk, _)| vk)
        }

        /// A printable character with no key on the layout, which has to be
        /// typed as text. Control characters keep their own keys.
        pub fn text_only(c: char) -> bool {
            !c.is_control() && lookup(c).is_none()
        }

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
            // The numeric keypad. The server sends these with the character they
            // make, which would type the digit row's key; the HID code says which
            // physical key it was. Windows virtual keys VK_NUMPAD0..9 (0x60..),
            // VK_MULTIPLY, VK_ADD, VK_SUBTRACT, VK_DECIMAL, VK_DIVIDE.
            #[cfg(windows)]
            0x54 => Key::Other(0x6F),
            #[cfg(windows)]
            0x55 => Key::Other(0x6A),
            #[cfg(windows)]
            0x56 => Key::Other(0x6D),
            #[cfg(windows)]
            0x57 => Key::Other(0x6B),
            #[cfg(windows)]
            0x59..=0x61 => Key::Other(0x61 + u32::from(hid - 0x59)),
            #[cfg(windows)]
            0x62 => Key::Other(0x60),
            #[cfg(windows)]
            0x63 => Key::Other(0x6E),
            // Scroll Lock is an ordinary key here (VK_SCROLL): it toggles the
            // client's own Scroll Lock state. The server's `scroll_lock_locks`
            // option is not wired to it.
            #[cfg(windows)]
            0x47 => Key::Other(0x91),
            // The Menu / Application key between Right Alt and Right Ctrl.
            // enigo only has it on Windows, so other clients still ignore it.
            #[cfg(windows)]
            0x65 => Key::Apps,
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
            // Sound and media: enigo has them on both systems.
            c::MUTE => Key::VolumeMute,
            c::VOLUME_DOWN => Key::VolumeDown,
            c::VOLUME_UP => Key::VolumeUp,
            c::PLAY_PAUSE => Key::MediaPlayPause,
            c::NEXT_TRACK => Key::MediaNextTrack,
            c::PREVIOUS_TRACK => Key::MediaPrevTrack,
            c::STOP => Key::MediaStop,
            // Launchers and browser keys have no enigo name, so they go by the system's
            // own code: a virtual key on Windows ...
            #[cfg(windows)]
            c::MAIL => Key::Other(0xB4),
            #[cfg(windows)]
            c::EXPLORER => Key::Other(0xB6),
            #[cfg(windows)]
            c::CALCULATOR => Key::Other(0xB7),
            #[cfg(windows)]
            c::BROWSER_BACK => Key::Other(0xA6),
            #[cfg(windows)]
            c::BROWSER_FORWARD => Key::Other(0xA7),
            #[cfg(windows)]
            c::BROWSER_REFRESH => Key::Other(0xA8),
            #[cfg(windows)]
            c::BROWSER_STOP => Key::Other(0xA9),
            #[cfg(windows)]
            c::BROWSER_SEARCH => Key::Other(0xAA),
            #[cfg(windows)]
            c::BROWSER_FAVORITES => Key::Other(0xAB),
            #[cfg(windows)]
            c::BROWSER_HOME => Key::Other(0xAC),
            // ... and an X11 keysym (XF86...) elsewhere. Brightness and microphone mute
            // are only reachable this way: Windows has no key for them to press.
            #[cfg(all(unix, not(target_os = "macos")))]
            c::BRIGHTNESS_UP => Key::Other(0x1008_FF02),
            #[cfg(all(unix, not(target_os = "macos")))]
            c::BRIGHTNESS_DOWN => Key::Other(0x1008_FF03),
            #[cfg(all(unix, not(target_os = "macos")))]
            c::MIC_MUTE => Key::Other(0x1008_FFB2),
            #[cfg(all(unix, not(target_os = "macos")))]
            c::MAIL => Key::Other(0x1008_FF19),
            #[cfg(all(unix, not(target_os = "macos")))]
            c::CALCULATOR => Key::Other(0x1008_FF1D),
            #[cfg(all(unix, not(target_os = "macos")))]
            c::EXPLORER => Key::Other(0x1008_FF5D),
            #[cfg(all(unix, not(target_os = "macos")))]
            c::BROWSER => Key::Other(0x1008_FF2E),
            #[cfg(all(unix, not(target_os = "macos")))]
            c::BROWSER_SEARCH => Key::Other(0x1008_FF1B),
            #[cfg(all(unix, not(target_os = "macos")))]
            c::BROWSER_HOME => Key::Other(0x1008_FF18),
            #[cfg(all(unix, not(target_os = "macos")))]
            c::BROWSER_BACK => Key::Other(0x1008_FF26),
            #[cfg(all(unix, not(target_os = "macos")))]
            c::BROWSER_FORWARD => Key::Other(0x1008_FF27),
            #[cfg(all(unix, not(target_os = "macos")))]
            c::BROWSER_STOP => Key::Other(0x1008_FF28),
            #[cfg(all(unix, not(target_os = "macos")))]
            c::BROWSER_REFRESH => Key::Other(0x1008_FF29),
            #[cfg(all(unix, not(target_os = "macos")))]
            c::BROWSER_FAVORITES => Key::Other(0x1008_FF30),
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
        fn volume_and_media_keys_are_named_keys() {
            assert_eq!(key_for(&ev(c::VOLUME_UP, None)), Some(Key::VolumeUp));
            assert_eq!(key_for(&ev(c::VOLUME_DOWN, None)), Some(Key::VolumeDown));
            assert_eq!(key_for(&ev(c::MUTE, None)), Some(Key::VolumeMute));
            assert_eq!(key_for(&ev(c::PLAY_PAUSE, None)), Some(Key::MediaPlayPause));
            assert_eq!(key_for(&ev(c::NEXT_TRACK, None)), Some(Key::MediaNextTrack));
        }

        #[cfg(windows)]
        #[test]
        fn browser_and_launcher_keys_are_virtual_keys_and_brightness_has_none_on_windows() {
            assert_eq!(key_for(&ev(c::BROWSER_BACK, None)), Some(Key::Other(0xA6)));
            assert_eq!(key_for(&ev(c::CALCULATOR, None)), Some(Key::Other(0xB7)));
            assert_eq!(key_for(&ev(c::BRIGHTNESS_UP, None)), None);
            assert_eq!(key_for(&ev(c::MIC_MUTE, None)), None);
        }

        #[cfg(all(unix, not(target_os = "macos")))]
        #[test]
        fn brightness_and_mic_mute_are_x11_keysyms_on_linux() {
            assert_eq!(
                key_for(&ev(c::BRIGHTNESS_UP, None)),
                Some(Key::Other(0x1008_FF02))
            );
            assert_eq!(
                key_for(&ev(c::BRIGHTNESS_DOWN, None)),
                Some(Key::Other(0x1008_FF03))
            );
            assert_eq!(
                key_for(&ev(c::MIC_MUTE, None)),
                Some(Key::Other(0x1008_FFB2))
            );
        }

        #[cfg(windows)]
        #[test]
        fn keypad_keys_are_numpad_virtual_keys_whatever_the_character() {
            // KP 1, KP 0, KP + and KP . with the character a server adds.
            assert_eq!(key_for(&ev(0x59, Some('1'))), Some(Key::Other(0x61)));
            assert_eq!(key_for(&ev(0x61, Some('9'))), Some(Key::Other(0x69)));
            assert_eq!(key_for(&ev(0x62, Some('0'))), Some(Key::Other(0x60)));
            assert_eq!(key_for(&ev(0x57, Some('+'))), Some(Key::Other(0x6B)));
            assert_eq!(key_for(&ev(0x63, Some('.'))), Some(Key::Other(0x6E)));
            // The digit row is untouched.
            assert_eq!(key_for(&ev(0x1E, Some('1'))), Some(Key::Unicode('1')));
        }

        #[cfg(windows)]
        #[test]
        fn scroll_lock_is_an_ordinary_key_on_windows() {
            assert_eq!(key_for(&ev(0x47, None)), Some(Key::Other(0x91)));
        }

        #[cfg(windows)]
        #[test]
        fn the_menu_key_is_the_apps_key_on_windows() {
            assert_eq!(key_for(&ev(0x65, None)), Some(Key::Apps));
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

        /// The lookups depend on the focused window's layout, so these only
        /// state what holds on every Latin layout (and on layouts without
        /// Latin letters, where nothing is found and everything is text).
        #[cfg(windows)]
        #[test]
        fn windows_lookups_never_hand_enigo_a_shift_state_as_a_key() {
            assert_eq!(win::shifted_vk('a'), None);
            assert!(matches!(win::shifted_vk('A'), None | Some(0x41)));
            // Control characters keep their keys and are never "text".
            assert!(!win::text_only('\n'));
            // `os_key` leaves keys that need no help alone.
            assert_eq!(os_key(Key::Return), Key::Return);
            assert_eq!(os_key(Key::Unicode('a')), Key::Unicode('a'));
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
    fn a_machines_own_setup_changes_only_the_special_keys() {
        use std::collections::HashMap;
        let perform = HashMap::from([
            (consumer::BRIGHTNESS_UP, Perform::Ignore),
            (consumer::MIC_MUTE, Perform::Keysym(0x1008_FFB2)),
            (consumer::MAIL, Perform::VirtualKey(0xB4)),
            (consumer::MUTE, Perform::Action(consumer::VOLUME_DOWN)),
            // A keyboard usage cannot be set up, even if the table says so.
            (0x04, Perform::Ignore),
        ]);
        assert_eq!(resolve(&perform, consumer::BRIGHTNESS_UP), Resolved::Ignore);
        assert_eq!(
            resolve(&perform, consumer::MIC_MUTE),
            Resolved::Keysym(0x1008_FFB2)
        );
        assert_eq!(
            resolve(&perform, consumer::MAIL),
            Resolved::VirtualKey(0xB4)
        );
        assert_eq!(
            resolve(&perform, consumer::MUTE),
            Resolved::Key(consumer::VOLUME_DOWN)
        );
        // Not mentioned: the built-in way.
        assert_eq!(
            resolve(&perform, consumer::VOLUME_UP),
            Resolved::Key(consumer::VOLUME_UP)
        );
        assert_eq!(resolve(&perform, 0x04), Resolved::Key(0x04));
        // Two actions that name each other do not loop: one step only.
        let loopy = HashMap::from([
            (consumer::MUTE, Perform::Action(consumer::VOLUME_UP)),
            (consumer::VOLUME_UP, Perform::Action(consumer::MUTE)),
        ]);
        assert_eq!(
            resolve(&loopy, consumer::MUTE),
            Resolved::Key(consumer::VOLUME_UP)
        );
    }

    #[test]
    fn null_sink_is_a_sink() {
        fn takes_sink(_: impl InputSink) {}
        takes_sink(NullSink);
    }
}
