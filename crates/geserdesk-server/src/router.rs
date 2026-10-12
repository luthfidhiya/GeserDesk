// SPDX-License-Identifier: GPL-3.0-or-later
//! Decides which screen owns the keyboard and mouse, and forwards input.
//!
//! [`Core`] is the whole decision procedure as a pure state machine: capture
//! events and connection changes go in, [`Action`]s come out. It does no I/O
//! and reads no clock (the caller passes a monotonic millisecond stamp), so
//! every scenario -- edge crossing, hotkey, a client vanishing mid-session --
//! is unit-tested without a display or a network. [`run`] is the thin async
//! shell that wires it to the capture backend and the client hub.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Instant;

use geserdesk_platform::{CaptureCmd, CaptureEvent, CaptureHandle};
use geserdesk_proto::{Edge, ModMask, MouseEvent, Point, Rect, ServerMsg};
use tokio::sync::mpsc::UnboundedReceiver;
use tracing::{debug, info, warn};

use crate::hub::{Hub, HubEvent};
use crate::layout::{Decision, Layout, SwitchPolicy, SwitchState};

/// Something the shell must do on behalf of [`Core`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Queue a message for a client.
    Send { to: String, msg: ServerMsg },
    /// Instruct the capture backend.
    Capture(CaptureCmd),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum State {
    /// The server's own screen has the keyboard and mouse.
    Local,
    /// `screen` has them; `pos` is the cursor's position on that screen.
    Remote { screen: String, pos: Point },
}

/// The routing state machine.
pub struct Core {
    layout: Layout,
    policy: SwitchPolicy,
    local: String,
    bounds: Rect,
    connected: HashMap<String, Rect>,
    state: State,
    switch: SwitchState,
    seq: u64,
}

impl Core {
    pub fn new(layout: Layout, policy: SwitchPolicy, local: String, bounds: Rect) -> Self {
        Self {
            layout,
            policy,
            local,
            bounds,
            connected: HashMap::new(),
            state: State::Local,
            switch: SwitchState::new(),
            seq: 0,
        }
    }

    /// The remote screen currently holding the input, if any.
    pub fn active_remote(&self) -> Option<&str> {
        match &self.state {
            State::Local => None,
            State::Remote { screen, .. } => Some(screen),
        }
    }

    pub fn client_connected(&mut self, name: &str, bounds: Rect) {
        self.connected.insert(name.to_string(), bounds);
    }

    /// A client went away. If it held the input, take it back.
    pub fn client_disconnected(&mut self, name: &str) -> Vec<Action> {
        self.connected.remove(name);
        match &self.state {
            State::Remote { screen, .. } if screen == name => {
                info!(
                    screen = name,
                    "active screen disconnected; input returns to local"
                );
                self.state = State::Local;
                self.switch.switched();
                vec![Action::Capture(CaptureCmd::Ungrab {
                    warp: self.bounds.center(),
                })]
            }
            _ => Vec::new(),
        }
    }

    pub fn handle(&mut self, ev: CaptureEvent, now_ms: u64) -> Vec<Action> {
        match ev {
            CaptureEvent::Pointer(p) => self.on_pointer(p, now_ms),
            CaptureEvent::Hotkey => self.on_hotkey(),
            CaptureEvent::GrabFailed => self.on_grab_failed(),
            CaptureEvent::Motion { dx, dy } => self.on_motion(dx, dy),
            CaptureEvent::Button { button, down } => {
                self.forward(ServerMsg::Mouse(MouseEvent::Button { button, down }))
            }
            CaptureEvent::Wheel { dx, dy } => {
                self.forward(ServerMsg::Mouse(MouseEvent::Wheel { dx, dy }))
            }
            CaptureEvent::Key(k) => self.forward(ServerMsg::Key(k)),
        }
    }

    // ---- local mode ---------------------------------------------------------

    fn on_pointer(&mut self, p: Point, now_ms: u64) -> Vec<Action> {
        if self.state != State::Local {
            return Vec::new(); // stale sample from before the grab
        }
        let jump = self.policy.jump_zone.max(1);

        let hit = Edge::ALL.into_iter().find_map(|edge| {
            if !at_edge(&self.bounds, p, edge, jump) {
                return None;
            }
            let target = self.layout.neighbor(&self.local, edge)?;
            self.connected
                .contains_key(target)
                .then(|| (edge, target.to_string()))
        });
        let Some((edge, target)) = hit else {
            self.switch.left_edge();
            return Vec::new();
        };

        let in_corner = near_corner(&self.bounds, p, edge, self.policy.corner_size);
        match self
            .switch
            .evaluate(&self.policy, now_ms, edge, true, in_corner)
        {
            Decision::Switch => {
                let to = self.connected[&target];
                let at = map_across(&self.bounds, p, edge, &to, 0);
                self.enter(target, at)
            }
            Decision::Wait | Decision::Blocked => Vec::new(),
        }
    }

    fn on_hotkey(&mut self) -> Vec<Action> {
        match &self.state {
            State::Remote { screen, .. } => {
                let screen = screen.clone();
                let at = self.bounds.center();
                self.go_local(&screen, at)
            }
            State::Local => {
                let target = Edge::ALL
                    .into_iter()
                    .filter_map(|e| self.layout.neighbor(&self.local, e))
                    .find(|n| self.connected.contains_key(*n))
                    .map(str::to_string);
                match target {
                    Some(t) => {
                        let at = self.connected[&t].center();
                        self.enter(t, at)
                    }
                    None => {
                        info!("hotkey pressed but no linked client is connected");
                        Vec::new()
                    }
                }
            }
        }
    }

    fn on_grab_failed(&mut self) -> Vec<Action> {
        match std::mem::replace(&mut self.state, State::Local) {
            State::Remote { screen, .. } => {
                warn!(
                    screen,
                    "could not grab local input; staying on the local screen"
                );
                self.switch.switched();
                vec![Action::Send {
                    to: screen,
                    msg: ServerMsg::Leave,
                }]
            }
            State::Local => Vec::new(),
        }
    }

    // ---- remote mode --------------------------------------------------------

    fn on_motion(&mut self, dx: i32, dy: i32) -> Vec<Action> {
        let State::Remote { screen, pos } = &self.state else {
            return Vec::new();
        };
        let screen = screen.clone();
        let Some(rb) = self.connected.get(&screen).copied() else {
            return Vec::new();
        };
        let moved = Point::new(pos.x + dx, pos.y + dy);

        let exit = if moved.x < rb.left() {
            Some(Edge::Left)
        } else if moved.x >= rb.right() {
            Some(Edge::Right)
        } else if moved.y < rb.top() {
            Some(Edge::Top)
        } else if moved.y >= rb.bottom() {
            Some(Edge::Bottom)
        } else {
            None
        };

        if let Some(edge) = exit {
            let edge_pos = rb.clamp(moved);
            match self.layout.neighbor(&screen, edge).map(str::to_string) {
                Some(n) if n == self.local => {
                    let jump = self.policy.jump_zone.max(1);
                    let at = map_across(&rb, edge_pos, edge, &self.bounds, jump + 1);
                    return self.go_local(&screen, at);
                }
                Some(n) if self.connected.contains_key(&n) => {
                    let to = self.connected[&n];
                    let at = map_across(&rb, edge_pos, edge, &to, 0);
                    let mut actions = vec![Action::Send {
                        to: screen,
                        msg: ServerMsg::Leave,
                    }];
                    actions.extend(self.enter_from_remote(n, at));
                    return actions;
                }
                _ => {} // no neighbour: the cursor stops at the edge
            }
        }

        let clamped = rb.clamp(moved);
        self.state = State::Remote {
            screen: screen.clone(),
            pos: clamped,
        };
        vec![Action::Send {
            to: screen,
            msg: ServerMsg::Mouse(MouseEvent::MoveAbs {
                x: clamped.x,
                y: clamped.y,
            }),
        }]
    }

    fn forward(&self, msg: ServerMsg) -> Vec<Action> {
        match &self.state {
            State::Remote { screen, .. } => vec![Action::Send {
                to: screen.clone(),
                msg,
            }],
            State::Local => Vec::new(),
        }
    }

    // ---- transitions --------------------------------------------------------

    /// Local -> remote: grab the devices and tell the client the cursor is there.
    fn enter(&mut self, target: String, at: Point) -> Vec<Action> {
        info!(screen = %target, x = at.x, y = at.y, "switching to remote screen");
        let mut actions = vec![Action::Capture(CaptureCmd::Grab)];
        actions.extend(self.enter_from_remote(target, at));
        actions
    }

    /// Hand the cursor to `target` (devices are, or are about to be, grabbed).
    fn enter_from_remote(&mut self, target: String, at: Point) -> Vec<Action> {
        self.seq += 1;
        self.switch.switched();
        self.state = State::Remote {
            screen: target.clone(),
            pos: at,
        };
        vec![Action::Send {
            to: target,
            msg: ServerMsg::Enter {
                x: at.x,
                y: at.y,
                seq: self.seq,
                toggle_mods: ModMask::empty(),
            },
        }]
    }

    /// Remote -> local at `at`.
    fn go_local(&mut self, from: &str, at: Point) -> Vec<Action> {
        info!(screen = from, "switching back to the local screen");
        self.state = State::Local;
        self.switch.switched();
        vec![
            Action::Send {
                to: from.to_string(),
                msg: ServerMsg::Leave,
            },
            Action::Capture(CaptureCmd::Ungrab { warp: at }),
        ]
    }
}

/// Is `p` within `jump` pixels of `edge` of `bounds`?
fn at_edge(bounds: &Rect, p: Point, edge: Edge, jump: i32) -> bool {
    match edge {
        Edge::Left => p.x < bounds.left() + jump,
        Edge::Right => p.x >= bounds.right() - jump,
        Edge::Top => p.y < bounds.top() + jump,
        Edge::Bottom => p.y >= bounds.bottom() - jump,
    }
}

/// Corner constraint: with `corner_size == 0` the whole edge counts; otherwise
/// only positions within `corner_size` pixels of either end of the edge.
fn near_corner(bounds: &Rect, p: Point, edge: Edge, corner_size: i32) -> bool {
    if corner_size <= 0 {
        return true;
    }
    let (pos, lo, hi) = match edge {
        Edge::Left | Edge::Right => (p.y, bounds.top(), bounds.bottom()),
        Edge::Top | Edge::Bottom => (p.x, bounds.left(), bounds.right()),
    };
    pos - lo < corner_size || hi - 1 - pos < corner_size
}

/// Where a cursor that crossed `edge` of `from` at `p` lands on `to`.
/// The coordinate along the edge is scaled proportionally; the other
/// coordinate is `inset` pixels inside the opposite side of `to`.
fn map_across(from: &Rect, p: Point, edge: Edge, to: &Rect, inset: i32) -> Point {
    fn scale(v: i32, f0: i32, flen: i32, t0: i32, tlen: i32) -> i32 {
        let num = i64::from((v - f0).clamp(0, (flen - 1).max(0))) * i64::from((tlen - 1).max(0));
        t0 + (num / i64::from((flen - 1).max(1))) as i32
    }
    match edge {
        Edge::Right => Point::new(to.left() + inset, scale(p.y, from.y, from.h, to.y, to.h)),
        Edge::Left => Point::new(
            to.right() - 1 - inset,
            scale(p.y, from.y, from.h, to.y, to.h),
        ),
        Edge::Bottom => Point::new(scale(p.x, from.x, from.w, to.x, to.w), to.top() + inset),
        Edge::Top => Point::new(
            scale(p.x, from.x, from.w, to.x, to.w),
            to.bottom() - 1 - inset,
        ),
    }
}

/// Drive [`Core`] from the capture backend and the client hub until either
/// input stream ends.
pub async fn run(
    mut core: Core,
    hub: Arc<Hub>,
    capture: CaptureHandle,
    mut capture_events: UnboundedReceiver<CaptureEvent>,
    mut hub_events: UnboundedReceiver<HubEvent>,
) {
    let started = Instant::now();
    loop {
        let mut pending: VecDeque<Action> = tokio::select! {
            ev = capture_events.recv() => match ev {
                Some(ev) => core.handle(ev, started.elapsed().as_millis() as u64).into(),
                None => break,
            },
            ev = hub_events.recv() => match ev {
                Some(HubEvent::Connected { name, bounds }) => {
                    debug!(%name, "client registered with the router");
                    core.client_connected(&name, bounds);
                    VecDeque::new()
                }
                Some(HubEvent::Disconnected(name)) => core.client_disconnected(&name).into(),
                None => break,
            },
        };

        while let Some(action) = pending.pop_front() {
            match action {
                Action::Capture(cmd) => {
                    if !capture.send(cmd) {
                        warn!("capture backend stopped; router exiting");
                        return;
                    }
                }
                Action::Send { to, msg } => {
                    if !hub.send(&to, msg) {
                        // The client vanished between events.
                        pending.extend(core.client_disconnected(&to));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use geserdesk_proto::{Button, KeyAction, KeyEvent};

    const LOCAL: Rect = Rect::new(0, 0, 1920, 1080);
    const REMOTE: Rect = Rect::new(0, 0, 2560, 1440);

    fn layout() -> Layout {
        // linux-pc <-> win <-> mac, left to right.
        let mut l = Layout::new(vec!["linux-pc".into(), "win".into(), "mac".into()]);
        l.link("linux-pc", Edge::Right, "win").unwrap();
        l.link("win", Edge::Left, "linux-pc").unwrap();
        l.link("win", Edge::Right, "mac").unwrap();
        l.link("mac", Edge::Left, "win").unwrap();
        l
    }

    fn core_with(policy: SwitchPolicy) -> Core {
        let mut c = Core::new(layout(), policy, "linux-pc".into(), LOCAL);
        c.client_connected("win", REMOTE);
        c
    }

    fn core() -> Core {
        core_with(SwitchPolicy::default())
    }

    fn sends(actions: &[Action]) -> Vec<(&str, &ServerMsg)> {
        actions
            .iter()
            .filter_map(|a| match a {
                Action::Send { to, msg } => Some((to.as_str(), msg)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn pointer_in_the_middle_does_nothing() {
        let mut c = core();
        assert!(c
            .handle(CaptureEvent::Pointer(Point::new(500, 500)), 0)
            .is_empty());
        assert_eq!(c.active_remote(), None);
    }

    #[test]
    fn pushing_the_right_edge_grabs_and_enters_the_neighbour() {
        let mut c = core();
        let a = c.handle(CaptureEvent::Pointer(Point::new(1919, 540)), 0);
        assert_eq!(a[0], Action::Capture(CaptureCmd::Grab));
        // y scales from the 1080-high local screen to the 1440-high remote one.
        match sends(&a)[0] {
            ("win", ServerMsg::Enter { x, y, seq, .. }) => {
                assert_eq!(*x, 0);
                assert!((*y - 719).abs() <= 1, "y was {y}");
                assert_eq!(*seq, 1);
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(c.active_remote(), Some("win"));
    }

    #[test]
    fn edge_without_a_connected_neighbour_is_ignored() {
        let mut c = Core::new(layout(), SwitchPolicy::default(), "linux-pc".into(), LOCAL);
        assert!(c
            .handle(CaptureEvent::Pointer(Point::new(1919, 10)), 0)
            .is_empty());
        // And an edge with no link at all:
        c.client_connected("win", REMOTE);
        assert!(c
            .handle(CaptureEvent::Pointer(Point::new(0, 10)), 0)
            .is_empty());
    }

    #[test]
    fn dwell_delay_holds_back_the_switch() {
        let mut c = core_with(SwitchPolicy {
            delay_ms: 200,
            ..SwitchPolicy::default()
        });
        let p = CaptureEvent::Pointer(Point::new(1919, 500));
        assert!(c.handle(p.clone(), 1000).is_empty());
        assert!(c.handle(p.clone(), 1100).is_empty());
        assert!(!c.handle(p, 1200).is_empty());
    }

    #[test]
    fn corner_constraint_limits_where_the_edge_switches() {
        let mut c = core_with(SwitchPolicy {
            corner_size: 100,
            ..SwitchPolicy::default()
        });
        assert!(c
            .handle(CaptureEvent::Pointer(Point::new(1919, 500)), 0)
            .is_empty());
        assert!(!c
            .handle(CaptureEvent::Pointer(Point::new(1919, 50)), 1)
            .is_empty());
    }

    #[test]
    fn scroll_lock_pins_the_cursor_to_the_local_screen() {
        let mut c = core();
        c.switch.set_locked(true);
        assert!(c
            .handle(CaptureEvent::Pointer(Point::new(1919, 500)), 0)
            .is_empty());
    }

    #[test]
    fn motion_is_forwarded_as_absolute_moves() {
        let mut c = core();
        c.handle(CaptureEvent::Pointer(Point::new(1919, 540)), 0);
        let a = c.handle(CaptureEvent::Motion { dx: 30, dy: -10 }, 1);
        match sends(&a)[0] {
            ("win", ServerMsg::Mouse(MouseEvent::MoveAbs { x, .. })) => assert_eq!(*x, 30),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn cursor_stops_at_an_edge_with_no_neighbour() {
        let mut c = core();
        c.handle(CaptureEvent::Pointer(Point::new(1919, 540)), 0);
        let a = c.handle(CaptureEvent::Motion { dx: 0, dy: -5000 }, 1);
        match sends(&a)[0] {
            ("win", ServerMsg::Mouse(MouseEvent::MoveAbs { y, .. })) => assert_eq!(*y, 0),
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(c.active_remote(), Some("win"));
    }

    #[test]
    fn pushing_back_through_the_link_returns_to_local() {
        let mut c = core();
        c.handle(CaptureEvent::Pointer(Point::new(1919, 540)), 0);
        let a = c.handle(CaptureEvent::Motion { dx: -5, dy: 0 }, 1);
        assert!(matches!(sends(&a)[0], ("win", ServerMsg::Leave)));
        match a.last().unwrap() {
            Action::Capture(CaptureCmd::Ungrab { warp }) => {
                // Placed just inside the right edge so it does not re-trigger.
                assert_eq!(warp.x, 1920 - 1 - 2);
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(c.active_remote(), None);
        // The local pointer sample at the warp target must not switch again.
        assert!(c
            .handle(CaptureEvent::Pointer(Point::new(1917, 540)), 2)
            .is_empty());
    }

    #[test]
    fn hopping_between_two_remote_screens_keeps_the_grab() {
        let mut c = core();
        c.client_connected("mac", Rect::new(0, 0, 1440, 900));
        c.handle(CaptureEvent::Pointer(Point::new(1919, 540)), 0);
        // Walk right across the whole 2560px remote screen.
        let a = c.handle(CaptureEvent::Motion { dx: 2600, dy: 0 }, 1);
        let s = sends(&a);
        assert!(matches!(s[0], ("win", ServerMsg::Leave)));
        assert!(matches!(s[1], ("mac", ServerMsg::Enter { x: 0, .. })));
        assert!(!a.iter().any(|x| matches!(x, Action::Capture(_))));
        assert_eq!(c.active_remote(), Some("mac"));
    }

    #[test]
    fn hotkey_toggles_there_and_back() {
        let mut c = core();
        let a = c.handle(CaptureEvent::Hotkey, 0);
        assert_eq!(a[0], Action::Capture(CaptureCmd::Grab));
        assert!(matches!(
            sends(&a)[0],
            (
                "win",
                ServerMsg::Enter {
                    x: 1280,
                    y: 720,
                    ..
                }
            )
        ));
        let b = c.handle(CaptureEvent::Hotkey, 1);
        assert!(matches!(sends(&b)[0], ("win", ServerMsg::Leave)));
        assert_eq!(
            b.last().unwrap(),
            &Action::Capture(CaptureCmd::Ungrab {
                warp: LOCAL.center()
            })
        );
        assert_eq!(c.active_remote(), None);
    }

    #[test]
    fn hotkey_with_nobody_connected_does_nothing() {
        let mut c = Core::new(layout(), SwitchPolicy::default(), "linux-pc".into(), LOCAL);
        assert!(c.handle(CaptureEvent::Hotkey, 0).is_empty());
    }

    #[test]
    fn keys_buttons_and_wheel_only_flow_while_remote() {
        let key = KeyEvent {
            hid: 4,
            ch: Some('a'),
            mods: ModMask::empty(),
            action: KeyAction::Down,
        };
        let mut c = core();
        assert!(c.handle(CaptureEvent::Key(key.clone()), 0).is_empty());
        assert!(c
            .handle(
                CaptureEvent::Button {
                    button: Button::Left,
                    down: true
                },
                0
            )
            .is_empty());

        c.handle(CaptureEvent::Hotkey, 1);
        let a = c.handle(CaptureEvent::Key(key.clone()), 2);
        assert_eq!(sends(&a), vec![("win", &ServerMsg::Key(key))]);
        let a = c.handle(CaptureEvent::Wheel { dx: 0, dy: 120 }, 3);
        assert!(matches!(
            sends(&a)[0],
            ("win", ServerMsg::Mouse(MouseEvent::Wheel { dy: 120, .. }))
        ));
    }

    #[test]
    fn disconnect_of_the_active_client_returns_input() {
        let mut c = core();
        c.handle(CaptureEvent::Hotkey, 0);
        let a = c.client_disconnected("win");
        assert_eq!(
            a,
            vec![Action::Capture(CaptureCmd::Ungrab {
                warp: LOCAL.center()
            })]
        );
        assert_eq!(c.active_remote(), None);
        // Disconnecting an idle client is a no-op.
        c.client_connected("win", REMOTE);
        assert!(c.client_disconnected("win").is_empty());
    }

    #[test]
    fn failed_grab_falls_back_to_local() {
        let mut c = core();
        c.handle(CaptureEvent::Hotkey, 0);
        let a = c.handle(CaptureEvent::GrabFailed, 1);
        assert!(matches!(sends(&a)[0], ("win", ServerMsg::Leave)));
        assert_eq!(c.active_remote(), None);
    }

    #[test]
    fn stale_pointer_samples_are_ignored_while_remote() {
        let mut c = core();
        c.handle(CaptureEvent::Hotkey, 0);
        assert!(c
            .handle(CaptureEvent::Pointer(Point::new(1919, 5)), 1)
            .is_empty());
    }

    #[test]
    fn seq_increases_with_each_entry() {
        let mut c = core();
        let first = c.handle(CaptureEvent::Hotkey, 0);
        c.handle(CaptureEvent::Hotkey, 1);
        let second = c.handle(CaptureEvent::Hotkey, 2);
        let seq = |a: &[Action]| match sends(a)[0].1 {
            ServerMsg::Enter { seq, .. } => *seq,
            _ => unreachable!(),
        };
        assert!(seq(&second) > seq(&first));
    }

    #[test]
    fn map_across_scales_along_the_edge() {
        let a = Rect::new(0, 0, 1000, 1000);
        let b = Rect::new(0, 0, 2000, 500);
        assert_eq!(
            map_across(&a, Point::new(999, 0), Edge::Right, &b, 0),
            Point::new(0, 0)
        );
        assert_eq!(
            map_across(&a, Point::new(999, 999), Edge::Right, &b, 0),
            Point::new(0, 499)
        );
        assert_eq!(
            map_across(&a, Point::new(0, 500), Edge::Left, &b, 3).x,
            2000 - 1 - 3
        );
        assert_eq!(
            map_across(&a, Point::new(999, 999), Edge::Bottom, &b, 0),
            Point::new(1999, 0)
        );
    }

    /// The whole pipeline short of X: fake capture events -> router -> hub ->
    /// client session -> a real TCP client.
    #[tokio::test]
    async fn router_delivers_captured_input_to_a_connected_client() {
        use crate::config::Config;
        use crate::net::accept_loop_with;
        use geserdesk_proto::{ClientMsg, MessageStream, ScreenInfo, PROTOCOL_VERSION};
        use tokio::net::TcpStream;

        let config = Config::parse(
            r#"
            [server]
            listen = "127.0.0.1:0"
            screen = "linux-pc"
            [[screens]]
            name = "linux-pc"
            [[screens]]
            name = "win"
            [links]
            linux-pc = { right = "win" }
            win = { left = "linux-pc" }
        "#,
        )
        .unwrap();

        let hub = Hub::new();
        let hub_events = hub.take_events().unwrap();
        let (cap_tx, cap_rx) = tokio::sync::mpsc::unbounded_channel();
        let (capture, cmds) = CaptureHandle::detached(LOCAL);
        let core = Core::new(
            config.layout.clone(),
            config.policy,
            config.local_screen.clone(),
            LOCAL,
        );
        tokio::spawn(run(core, Arc::clone(&hub), capture, cap_rx, hub_events));

        let (_handle, listener) = crate::net::bind(&config).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (_, never) = tokio::sync::watch::channel(false);
        tokio::spawn(accept_loop_with(config, listener, hub, never));

        let mut ms = MessageStream::new(TcpStream::connect(addr).await.unwrap());
        ms.send(&ClientMsg::Hello {
            version: PROTOCOL_VERSION,
            screen: ScreenInfo {
                name: "win".into(),
                bounds: REMOTE,
                cursor: Point::new(0, 0),
            },
        })
        .await
        .unwrap();

        // The router learns of the client asynchronously; keep pushing the edge
        // until the switch happens.
        let mut got_enter = None;
        for _ in 0..50 {
            cap_tx
                .send(CaptureEvent::Pointer(Point::new(1919, 540)))
                .unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            if let Ok(CaptureCmd::Grab) = cmds.try_recv() {
                got_enter = Some(());
                break;
            }
        }
        assert!(got_enter.is_some(), "router never asked for a grab");

        let key = KeyEvent {
            hid: 4,
            ch: Some('a'),
            mods: ModMask::empty(),
            action: KeyAction::Down,
        };
        cap_tx.send(CaptureEvent::Key(key.clone())).unwrap();
        cap_tx.send(CaptureEvent::Motion { dx: 7, dy: 0 }).unwrap();

        let mut saw = (false, false, false);
        while !(saw.0 && saw.1 && saw.2) {
            match ms.recv::<ServerMsg>().await.unwrap() {
                ServerMsg::Enter { x: 0, .. } => saw.0 = true,
                ServerMsg::Key(k) if k == key => saw.1 = true,
                ServerMsg::Mouse(MouseEvent::MoveAbs { x: 7, .. }) => saw.2 = true,
                ServerMsg::KeepAlive => ms.send(&ClientMsg::KeepAlive).await.unwrap(),
                _ => {}
            }
        }

        // Dropping the connection hands the input back.
        drop(ms);
        let ungrab = tokio::task::spawn_blocking(move || {
            cmds.recv_timeout(std::time::Duration::from_secs(5))
        })
        .await
        .unwrap();
        assert!(
            matches!(ungrab, Ok(CaptureCmd::Ungrab { .. })),
            "{ungrab:?}"
        );
    }
}
