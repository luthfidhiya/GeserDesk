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
    use enigo::{Axis, Button as EBtn, Coordinate, Direction, Enigo, Keyboard, Mouse, Settings};
    use geserdesk_proto::Button;
    use std::collections::HashSet;

    /// Real OS input injection.
    ///
    /// Tracks which keys/buttons it has pressed so [`leave`](InputSink::leave)
    /// and drop can release them -- the same "release by physical key" concern
    /// that motivates carrying the HID code in [`KeyEvent`].
    pub struct EnigoSink {
        enigo: Enigo,
        held_keys: HashSet<u16>,
        held_buttons: HashSet<u8>,
    }

    impl EnigoSink {
        pub fn new() -> anyhow::Result<Self> {
            let enigo = Enigo::new(&Settings::default())
                .map_err(|e| anyhow::anyhow!("initialising enigo: {e}"))?;
            Ok(Self {
                enigo,
                held_keys: HashSet::new(),
                held_buttons: HashSet::new(),
            })
        }

        fn press_key(&mut self, ev: &KeyEvent, dir: Direction) {
            // M3: character-based path only. The HID->keysym path that makes
            // mismatched layouts work is M6.
            let key = match ev.ch {
                Some(c) => enigo::Key::Unicode(c),
                None => {
                    tracing::warn!(hid = ev.hid, "no character for key; HID mapping is M6");
                    return;
                }
            };
            if let Err(e) = self.enigo.key(key, dir) {
                tracing::warn!(error = %e, "key injection failed");
            }
        }
    }

    impl InputSink for EnigoSink {
        fn enter(&mut self, at: Point) {
            if let Err(e) = self.enigo.move_mouse(at.x, at.y, Coordinate::Abs) {
                tracing::warn!(error = %e, "move_mouse on enter failed");
            }
        }

        fn leave(&mut self) {
            let keys: Vec<u16> = self.held_keys.drain().collect();
            for hid in keys {
                let ev = KeyEvent {
                    hid,
                    ch: None,
                    mods: geserdesk_proto::ModMask::empty(),
                    action: KeyAction::Up,
                };
                self.press_key(&ev, Direction::Release);
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
                KeyAction::Down => {
                    self.held_keys.insert(ev.hid);
                    self.press_key(ev, Direction::Press);
                }
                KeyAction::Up => {
                    self.held_keys.remove(&ev.hid);
                    self.press_key(ev, Direction::Release);
                }
                KeyAction::Repeat(_) => self.press_key(ev, Direction::Press),
            }
        }

        fn mouse(&mut self, ev: MouseEvent) {
            let r = match ev {
                MouseEvent::MoveAbs { x, y } => self.enigo.move_mouse(x, y, Coordinate::Abs),
                MouseEvent::MoveRel { dx, dy } => self.enigo.move_mouse(dx, dy, Coordinate::Rel),
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
                        r = self.enigo.scroll(dx / 120, Axis::Horizontal);
                    }
                    if dy != 0 && r.is_ok() {
                        r = self.enigo.scroll(-dy / 120, Axis::Vertical);
                    }
                    r
                }
            };
            if let Err(e) = r {
                tracing::warn!(error = %e, "mouse injection failed");
            }
        }
    }

    impl Drop for EnigoSink {
        fn drop(&mut self) {
            self.leave();
        }
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
