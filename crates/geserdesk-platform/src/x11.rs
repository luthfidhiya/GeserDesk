// SPDX-License-Identifier: GPL-3.0-or-later
//! X11 capture backend (`x11rb`, no `unsafe`).
//!
//! Two modes, switched by [`CaptureCmd`]:
//!
//! * **Local** -- the pointer belongs to this machine. We poll its position
//!   (a cheap `QueryPointer` every couple of milliseconds) and report it so the
//!   server can notice a push against a screen edge. The same poll watches the
//!   physical key state (`QueryKeymap`) for `Ctrl+Alt+Home` and reports
//!   [`CaptureEvent::Hotkey`]. (A passive `XGrabKey` would swallow the keys, but
//!   it depends on the desktop not holding the combination; polling does not.)
//! * **Grabbed** -- another screen is active. We grab the pointer and keyboard
//!   with a blank cursor, park the pointer in the middle of the screen, and
//!   turn every motion into a delta by warping back to the middle. This is the
//!   classic software-KVM technique; the ordering subtleties around the warp
//!   are explained at [`Capture::on_motion`].

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Duration;

use geserdesk_proto::{Button, KeyAction, KeyEvent, ModMask, Point, Rect};
use tokio::sync::mpsc::UnboundedSender;
use tracing::{debug, error, info, warn};
use x11rb::connection::Connection;
use x11rb::protocol::xkb::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{
    ConnectionExt as _, CreateGCAux, EventMask, GrabMode, GrabStatus, KeyButMask, Rectangle, Window,
};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::{CURRENT_TIME, NONE};

use crate::keymap::{evdev_to_hid, hid_is_printable};
use crate::{CaptureCmd, CaptureError, CaptureEvent, CaptureHandle};

/// X11 keysym of Home, the hotkey's key. The modifiers are Ctrl+Alt.
///
/// Not F1-F12: Xorg itself turns Ctrl+Alt+Fn into a virtual-terminal switch and
/// never delivers those presses to clients.
const KEYSYM_HOTKEY: u32 = 0xff50;
/// How long the backend sleeps when there was nothing to do.
const IDLE_SLEEP: Duration = Duration::from_millis(2);

/// Connect to the X server and start the capture thread.
pub fn start(events: UnboundedSender<CaptureEvent>) -> Result<CaptureHandle, CaptureError> {
    let (conn, screen_num) =
        x11rb::connect(None).map_err(|e| CaptureError::Connect(e.to_string()))?;
    let screen = &conn.setup().roots[screen_num];
    let bounds = Rect::new(
        0,
        0,
        i32::from(screen.width_in_pixels),
        i32::from(screen.height_in_pixels),
    );
    let root = screen.root;

    let (cmd_tx, cmd_rx) = mpsc::channel();
    let mut capture =
        Capture::new(conn, root, bounds, events, cmd_rx).map_err(CaptureError::Connect)?;
    std::thread::Builder::new()
        .name("x11-capture".into())
        .spawn(move || {
            if let Err(e) = capture.run() {
                error!(error = %e, "X11 capture thread stopped");
            }
        })
        .map_err(|e| CaptureError::Connect(e.to_string()))?;

    info!(?bounds, "X11 capture started (hotkey: Ctrl+Alt+Home)");
    Ok(CaptureHandle::new(bounds, cmd_tx))
}

struct Keymap {
    min_keycode: u8,
    per_keycode: usize,
    syms: Vec<u32>,
}

/// Which physical keys make up the screen-switch chord.
#[derive(Debug)]
struct Hotkey {
    ctrl: Vec<u8>,
    alt: Vec<u8>,
    key: Option<u8>,
}

impl Hotkey {
    fn from_keymap(km: &Keymap) -> Self {
        let both = |a: u32, b: u32| {
            let mut v = km.keycodes_for(a);
            v.extend(km.keycodes_for(b));
            v
        };
        Self {
            ctrl: both(0xffe3, 0xffe4), // Control_L, Control_R
            alt: both(0xffe9, 0xffea),  // Alt_L, Alt_R
            key: km.keycode_for(KEYSYM_HOTKEY),
        }
    }

    /// Is the chord down, given the 32-byte `QueryKeymap` bitmap?
    fn is_down(&self, keys: &[u8; 32]) -> bool {
        let held = |code: u8| keys[usize::from(code / 8)] & (1 << (code % 8)) != 0;
        self.key.is_some_and(held)
            && self.ctrl.iter().copied().any(held)
            && self.alt.iter().copied().any(held)
    }
}

struct Capture {
    conn: RustConnection,
    root: Window,
    bounds: Rect,
    events: UnboundedSender<CaptureEvent>,
    cmds: Receiver<CaptureCmd>,
    blank_cursor: u32,
    keymap: Keymap,
    hotkey: Hotkey,
    /// The hotkey chord was down at the last poll (edge detection).
    hotkey_down: bool,

    grabbed: bool,
    center: Point,
    /// Last pointer position seen while grabbed.
    last: Point,
    /// A warp to `center` is in flight; events that predate it are still queued.
    expecting_center: bool,
    /// Keys currently held (X keycode -> repeat count), for auto-repeat.
    pressed: HashMap<u8, u16>,
    last_local: Option<Point>,
}

impl Capture {
    fn new(
        conn: RustConnection,
        root: Window,
        bounds: Rect,
        events: UnboundedSender<CaptureEvent>,
        cmds: Receiver<CaptureCmd>,
    ) -> Result<Self, String> {
        let e = |e: &dyn std::fmt::Display| e.to_string();

        // A fully transparent 1x1 cursor for the grabbed state.
        let pixmap = conn.generate_id().map_err(|x| e(&x))?;
        conn.create_pixmap(1, pixmap, root, 1, 1)
            .map_err(|x| e(&x))?;
        let gc = conn.generate_id().map_err(|x| e(&x))?;
        conn.create_gc(gc, pixmap, &CreateGCAux::new().foreground(0))
            .map_err(|x| e(&x))?;
        conn.poly_fill_rectangle(
            pixmap,
            gc,
            &[Rectangle {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            }],
        )
        .map_err(|x| e(&x))?;
        let blank_cursor = conn.generate_id().map_err(|x| e(&x))?;
        conn.create_cursor(blank_cursor, pixmap, pixmap, 0, 0, 0, 0, 0, 0, 0, 0)
            .map_err(|x| e(&x))?;
        conn.free_gc(gc).map_err(|x| e(&x))?;
        conn.free_pixmap(pixmap).map_err(|x| e(&x))?;

        // With detectable auto-repeat a held key yields repeated presses but no
        // synthetic releases, so we can tell repeats from real taps.
        match conn.xkb_use_extension(1, 0).map_err(|x| e(&x))?.reply() {
            Ok(r) if r.supported => {
                let flag = xkb::PerClientFlag::DETECTABLE_AUTO_REPEAT;
                let none = xkb::BoolCtrl::from(0u8);
                let _ = conn.xkb_per_client_flags(
                    xkb::ID::USE_CORE_KBD.into(),
                    flag,
                    flag,
                    none,
                    none,
                    none,
                );
            }
            _ => warn!("XKB unavailable; key auto-repeat will arrive as release+press"),
        }

        let keymap = load_keymap(&conn)?;
        let hotkey = Hotkey::from_keymap(&keymap);
        if hotkey.key.is_none() {
            warn!("no Home key in the keymap; the screen-switch hotkey is unavailable");
        }
        conn.flush().map_err(|x| e(&x))?;

        Ok(Self {
            conn,
            root,
            bounds,
            events,
            cmds,
            blank_cursor,
            keymap,
            hotkey,
            hotkey_down: false,
            grabbed: false,
            center: bounds.center(),
            last: bounds.center(),
            expecting_center: false,
            pressed: HashMap::new(),
            last_local: None,
        })
    }

    fn run(&mut self) -> Result<(), String> {
        loop {
            loop {
                match self.cmds.try_recv() {
                    Ok(cmd) => self.on_cmd(cmd),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        // The server dropped its handle: give the devices back.
                        if self.grabbed {
                            self.release(None);
                        }
                        return Ok(());
                    }
                }
            }

            let mut busy = false;
            while let Some(ev) = self.conn.poll_for_event().map_err(|e| e.to_string())? {
                busy = true;
                if !self.on_event(ev) {
                    return Ok(());
                }
            }

            if !self.grabbed {
                let keys = self.conn.query_keymap().map_err(|e| e.to_string())?;
                if let Ok(keys) = keys.reply() {
                    let down = self.hotkey.is_down(&keys.keys);
                    if down && !self.hotkey_down {
                        busy = true;
                        if !self.emit(CaptureEvent::Hotkey) {
                            return Ok(());
                        }
                    }
                    self.hotkey_down = down;
                }
                let p = self
                    .conn
                    .query_pointer(self.root)
                    .map_err(|e| e.to_string())?;
                if let Ok(p) = p.reply() {
                    let pos = Point::new(i32::from(p.root_x), i32::from(p.root_y));
                    if self.last_local != Some(pos) {
                        self.last_local = Some(pos);
                        busy = true;
                        if !self.emit(CaptureEvent::Pointer(pos)) {
                            return Ok(());
                        }
                    }
                }
            }

            if !busy {
                std::thread::sleep(IDLE_SLEEP);
            }
        }
    }

    /// Returns `false` once the receiving side is gone.
    fn emit(&self, ev: CaptureEvent) -> bool {
        self.events.send(ev).is_ok()
    }

    fn on_cmd(&mut self, cmd: CaptureCmd) {
        match cmd {
            CaptureCmd::Grab => {
                if self.grabbed {
                    return;
                }
                if let Err(e) = self.grab() {
                    warn!(error = %e, "grab failed");
                    let _ = self.emit(CaptureEvent::GrabFailed);
                }
            }
            CaptureCmd::Ungrab { warp } => self.release(Some(warp)),
        }
    }

    fn grab(&mut self) -> Result<(), String> {
        let mask = EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE | EventMask::POINTER_MOTION;
        let mut got_pointer = false;
        let mut status = GrabStatus::SUCCESS;
        for _ in 0..20 {
            let r = self
                .conn
                .grab_pointer(
                    false,
                    self.root,
                    mask,
                    GrabMode::ASYNC,
                    GrabMode::ASYNC,
                    NONE,
                    self.blank_cursor,
                    CURRENT_TIME,
                )
                .map_err(|e| e.to_string())?
                .reply()
                .map_err(|e| e.to_string())?;
            status = r.status;
            if status == GrabStatus::SUCCESS {
                got_pointer = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        if !got_pointer {
            return Err(format!("cannot grab the pointer (X status {status:?})"));
        }

        let kb = self
            .conn
            .grab_keyboard(
                false,
                self.root,
                CURRENT_TIME,
                GrabMode::ASYNC,
                GrabMode::ASYNC,
            )
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?;
        if kb.status != GrabStatus::SUCCESS {
            let _ = self.conn.ungrab_pointer(CURRENT_TIME);
            let _ = self.conn.flush();
            return Err("keyboard is grabbed by another program".into());
        }

        // Events already queued by the server carry the pointer's *current*
        // position, so deltas must start from there, not from the centre we
        // are about to warp to (see `on_motion`).
        self.last = self
            .conn
            .query_pointer(self.root)
            .ok()
            .and_then(|c| c.reply().ok())
            .map(|p| Point::new(i32::from(p.root_x), i32::from(p.root_y)))
            .unwrap_or(self.center);
        self.warp(self.center);
        self.expecting_center = true;
        self.grabbed = true;
        self.pressed.clear();
        info!("grabbed pointer and keyboard");
        Ok(())
    }

    fn release(&mut self, warp_to: Option<Point>) {
        let _ = self.conn.ungrab_keyboard(CURRENT_TIME);
        let _ = self.conn.ungrab_pointer(CURRENT_TIME);
        if let Some(p) = warp_to {
            self.warp(p);
            self.last_local = Some(p);
        }
        let _ = self.conn.flush();
        self.grabbed = false;
        self.pressed.clear();
        // If the chord is still held, do not read it as a fresh press.
        self.hotkey_down = true;
        debug!("released pointer and keyboard");
    }

    fn warp(&self, p: Point) {
        let p = self.bounds.clamp(p);
        let _ = self
            .conn
            .warp_pointer(NONE, self.root, 0, 0, 0, 0, p.x as i16, p.y as i16);
        let _ = self.conn.flush();
    }

    /// Returns `false` once the receiving side is gone.
    fn on_event(&mut self, ev: Event) -> bool {
        tracing::trace!(?ev, "x11 event");
        match ev {
            Event::MotionNotify(m) if self.grabbed => {
                self.on_motion(Point::new(i32::from(m.root_x), i32::from(m.root_y)))
            }
            Event::ButtonPress(b) if self.grabbed => self.on_button(b.detail, true),
            Event::ButtonRelease(b) if self.grabbed => self.on_button(b.detail, false),
            Event::KeyPress(k) => self.on_key(k.detail, k.state, true),
            Event::KeyRelease(k) => self.on_key(k.detail, k.state, false),
            Event::MappingNotify(_) => {
                match load_keymap(&self.conn) {
                    Ok(km) => self.keymap = km,
                    Err(e) => warn!(error = %e, "reloading keymap failed"),
                }
                true
            }
            _ => true,
        }
    }

    /// Turn an absolute pointer position into a delta.
    ///
    /// After each report we warp the pointer back to the centre so it never
    /// runs out of room. Motion events queued *before* the warp still carry
    /// pre-warp coordinates, so while a warp is in flight (`expecting_center`)
    /// we measure deltas against the previous event instead of the centre, and
    /// resume centre-relative tracking once the warp's own event (a motion to
    /// exactly the centre) arrives. Measuring against the centre too early
    /// would count the same movement twice.
    fn on_motion(&mut self, pos: Point) -> bool {
        if self.expecting_center {
            if pos == self.center {
                self.expecting_center = false;
                self.last = pos;
                return true;
            }
            let (dx, dy) = (pos.x - self.last.x, pos.y - self.last.y);
            self.last = pos;
            return dx == 0 && dy == 0 || self.emit(CaptureEvent::Motion { dx, dy });
        }

        let (dx, dy) = (pos.x - self.center.x, pos.y - self.center.y);
        if dx == 0 && dy == 0 {
            return true;
        }
        self.last = pos;
        self.expecting_center = true;
        self.warp(self.center);
        self.emit(CaptureEvent::Motion { dx, dy })
    }

    fn on_button(&mut self, detail: u8, down: bool) -> bool {
        let ev = match detail {
            1 => CaptureEvent::Button {
                button: Button::Left,
                down,
            },
            2 => CaptureEvent::Button {
                button: Button::Middle,
                down,
            },
            3 => CaptureEvent::Button {
                button: Button::Right,
                down,
            },
            // Scroll wheels arrive as buttons 4-7; one notch per press.
            4..=7 => {
                if !down {
                    return true;
                }
                let (dx, dy) = match detail {
                    4 => (0, 120),
                    5 => (0, -120),
                    6 => (-120, 0),
                    _ => (120, 0),
                };
                CaptureEvent::Wheel { dx, dy }
            }
            8 => CaptureEvent::Button {
                button: Button::Back,
                down,
            },
            9 => CaptureEvent::Button {
                button: Button::Forward,
                down,
            },
            n => CaptureEvent::Button {
                button: Button::Extra(n.saturating_sub(9)),
                down,
            },
        };
        self.emit(ev)
    }

    fn on_key(&mut self, keycode: u8, state: KeyButMask, down: bool) -> bool {
        let state = u16::from(state);
        debug!(keycode, state, down, hotkey = ?self.hotkey, grabbed = self.grabbed, "key event");
        let want = u16::from(KeyButMask::CONTROL | KeyButMask::MOD1);
        if Some(keycode) == self.hotkey.key && state & want == want {
            // Swallow both edges of the hotkey; report only the press.
            return !down || self.emit(CaptureEvent::Hotkey);
        }
        if !self.grabbed {
            return true;
        }

        let Some(hid) = keycode
            .checked_sub(8)
            .and_then(|c| evdev_to_hid(u16::from(c)))
        else {
            debug!(keycode, "unmapped key ignored");
            return true;
        };

        let action = if down {
            match self.pressed.get_mut(&keycode) {
                Some(n) => {
                    *n = n.saturating_add(1);
                    KeyAction::Repeat(*n)
                }
                None => {
                    self.pressed.insert(keycode, 0);
                    KeyAction::Down
                }
            }
        } else {
            self.pressed.remove(&keycode);
            KeyAction::Up
        };

        let ch = if hid_is_printable(hid) {
            self.keymap.char_for(keycode, state)
        } else {
            None
        };
        self.emit(CaptureEvent::Key(KeyEvent {
            hid,
            ch,
            mods: mods_from_state(state),
            action,
        }))
    }
}

fn load_keymap(conn: &RustConnection) -> Result<Keymap, String> {
    let setup = conn.setup();
    let (min, max) = (setup.min_keycode, setup.max_keycode);
    let reply = conn
        .get_keyboard_mapping(min, max - min + 1)
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| e.to_string())?;
    Ok(Keymap {
        min_keycode: min,
        per_keycode: usize::from(reply.keysyms_per_keycode),
        syms: reply.keysyms,
    })
}

impl Keymap {
    fn sym(&self, keycode: u8, index: usize) -> u32 {
        if keycode < self.min_keycode || index >= self.per_keycode {
            return 0;
        }
        let base = usize::from(keycode - self.min_keycode) * self.per_keycode;
        self.syms.get(base + index).copied().unwrap_or(0)
    }

    /// Every keycode whose unshifted symbol is `keysym`.
    fn keycodes_for(&self, keysym: u32) -> Vec<u8> {
        (self.min_keycode..=u8::MAX)
            .filter(|&k| self.sym(k, 0) == keysym)
            .collect()
    }

    /// The first keycode whose unshifted symbol is `keysym`.
    fn keycode_for(&self, keysym: u32) -> Option<u8> {
        (self.min_keycode..=u8::MAX).find(|&k| self.sym(k, 0) == keysym)
    }

    /// The character `keycode` produces under the modifier `state`.
    fn char_for(&self, keycode: u8, state: u16) -> Option<char> {
        let shift = state & 0x01 != 0;
        let lock = state & 0x02 != 0;
        let numlock = state & 0x10 != 0;
        let altgr = state & 0x80 != 0;

        let base_sym = self.sym(keycode, 0);
        let is_keypad = (0xff80..=0xffbd).contains(&base_sym);
        let is_alpha = keysym_to_char(base_sym).is_some_and(char::is_alphabetic);

        let level = if is_keypad {
            usize::from(numlock != shift)
        } else if altgr && self.per_keycode >= 6 {
            4 + usize::from(shift)
        } else if is_alpha {
            usize::from(shift != lock)
        } else {
            usize::from(shift)
        };
        let mut sym = self.sym(keycode, level);
        if sym == 0 {
            sym = base_sym;
        }
        keysym_to_char(sym)
    }
}

/// Printable X keysyms to characters (Latin-1, Unicode keysyms, keypad).
fn keysym_to_char(sym: u32) -> Option<char> {
    match sym {
        0x20..=0x7e | 0xa0..=0xff => char::from_u32(sym),
        0x0100_0000..=0x0110_ffff => char::from_u32(sym & 0x00ff_ffff),
        0xffb0..=0xffb9 => char::from_u32(u32::from(b'0') + (sym - 0xffb0)),
        0xffaa => Some('*'),
        0xffab => Some('+'),
        0xffac => Some(','),
        0xffad => Some('-'),
        0xffae => Some('.'),
        0xffaf => Some('/'),
        0xffbd => Some('='),
        _ => None,
    }
}

fn mods_from_state(state: u16) -> ModMask {
    let mut m = ModMask::empty();
    m.set(ModMask::SHIFT, state & 0x01 != 0);
    m.set(ModMask::CAPS_LOCK, state & 0x02 != 0);
    m.set(ModMask::CONTROL, state & 0x04 != 0);
    m.set(ModMask::ALT, state & 0x08 != 0);
    m.set(ModMask::NUM_LOCK, state & 0x10 != 0);
    m.set(ModMask::SUPER, state & 0x40 != 0);
    m.set(ModMask::ALT_GR, state & 0x80 != 0);
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keymap() -> Keymap {
        // keycode 38 = 'a'/'A', 10 = '1'/'!', 79 = KP_Home/KP_7, 20 = AltGr level.
        let per = 2;
        let mut syms = vec![0u32; 256 * per];
        let mut put = |k: usize, a: u32, b: u32| {
            syms[(k - 8) * per] = a;
            syms[(k - 8) * per + 1] = b;
        };
        put(38, 'a' as u32, 'A' as u32);
        put(10, '1' as u32, '!' as u32);
        put(79, 0xff95, 0xffb7);
        Keymap {
            min_keycode: 8,
            per_keycode: per,
            syms,
        }
    }

    #[test]
    fn letters_follow_shift_and_caps_lock() {
        let km = keymap();
        assert_eq!(km.char_for(38, 0), Some('a'));
        assert_eq!(km.char_for(38, 0x01), Some('A')); // shift
        assert_eq!(km.char_for(38, 0x02), Some('A')); // caps lock
        assert_eq!(km.char_for(38, 0x03), Some('a')); // both cancel
    }

    #[test]
    fn caps_lock_does_not_shift_digits() {
        let km = keymap();
        assert_eq!(km.char_for(10, 0x02), Some('1'));
        assert_eq!(km.char_for(10, 0x01), Some('!'));
    }

    #[test]
    fn keypad_follows_num_lock() {
        let km = keymap();
        assert_eq!(km.char_for(79, 0x10), Some('7')); // num lock on
        assert_eq!(km.char_for(79, 0x00), None); // KP_Home, not a character
    }

    #[test]
    fn hotkey_needs_the_whole_chord() {
        let hk = Hotkey {
            ctrl: vec![37, 105],
            alt: vec![64],
            key: Some(96),
        };
        let bitmap = |codes: &[u8]| {
            let mut m = [0u8; 32];
            for &c in codes {
                m[usize::from(c / 8)] |= 1 << (c % 8);
            }
            m
        };
        assert!(hk.is_down(&bitmap(&[37, 64, 96])));
        assert!(hk.is_down(&bitmap(&[105, 64, 96, 50]))); // right Ctrl, extra key
        assert!(!hk.is_down(&bitmap(&[37, 96]))); // no Alt
        assert!(!hk.is_down(&bitmap(&[64, 96]))); // no Ctrl
        assert!(!hk.is_down(&bitmap(&[37, 64]))); // no Home
    }

    #[test]
    fn keycode_lookup_by_keysym() {
        let km = keymap();
        assert_eq!(km.keycode_for('a' as u32), Some(38));
        assert_eq!(km.keycode_for(0xdead), None);
    }

    #[test]
    fn keysym_conversion() {
        assert_eq!(keysym_to_char(0x41), Some('A'));
        assert_eq!(keysym_to_char(0xe9), Some('é'));
        assert_eq!(keysym_to_char(0x0100_20ac), Some('€'));
        assert_eq!(keysym_to_char(0xffb5), Some('5'));
        assert_eq!(keysym_to_char(0xff0d), None); // Return
    }

    #[test]
    fn state_to_modmask() {
        let m = mods_from_state(0x01 | 0x04 | 0x40);
        assert!(m.contains(ModMask::SHIFT));
        assert!(m.contains(ModMask::CONTROL));
        assert!(m.contains(ModMask::SUPER));
        assert!(!m.contains(ModMask::ALT));
    }
}
