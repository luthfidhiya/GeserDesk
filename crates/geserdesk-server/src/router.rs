// SPDX-License-Identifier: GPL-3.0-or-later
//! Decides which screen owns the keyboard and mouse, and forwards input.
//!
//! [`Core`] is the whole decision procedure as a pure state machine: capture
//! events and connection changes go in, [`Action`]s come out. It does no I/O
//! and reads no clock (the caller passes a monotonic millisecond stamp), so
//! every scenario -- edge crossing, hotkey, a client vanishing mid-session --
//! is unit-tested without a display or a network. [`run`] is the thin async
//! shell that wires it to the capture backend and the client hub.
//!
//! Geometry lives on the shared virtual desk (see [`crate::desk`]): each machine
//! has an origin, each monitor a rectangle. The cursor crosses to another
//! machine exactly where a monitor edge touches a monitor of that machine, and
//! arrives at the physically corresponding spot, offsets included.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Arc;
use std::time::Instant;

use geserdesk_platform::{CaptureCmd, CaptureEvent, CaptureHandle};
use geserdesk_proto::{Edge, ModMask, MouseEvent, Point, Rect, ServerMsg, Size};
use tokio::sync::mpsc::UnboundedReceiver;
use tracing::{debug, info, warn};

use crate::desk::{desk_size, effective_origins, inset_clamp, nearest_point, Frame};
use crate::files::{self, LocalFiles, Owner};
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
    /// `screen` has them; `pos` is the cursor's position in that machine's own
    /// coordinates.
    Remote { screen: String, pos: Point },
}

/// The routing state machine.
pub struct Core {
    layout: Layout,
    policy: SwitchPolicy,
    local: String,
    local_monitors: Vec<Rect>,
    positions: BTreeMap<String, Point>,
    /// Desk sizes that differ from a machine's real one (see [`Frame`]).
    sizes: BTreeMap<String, Size>,
    /// Effective origin of every configured screen on the virtual desk.
    origins: HashMap<String, Point>,
    connected: HashMap<String, Vec<Rect>>,
    state: State,
    /// The local monitor the pointer was last seen on while the server had the
    /// input: where it comes back to when it is taken back with no edge to say
    /// where (the hotkey, or the client it was on going away).
    last_local_monitor: Option<Rect>,
    switch: SwitchState,
    seq: u64,
}

impl Core {
    pub fn new(
        layout: Layout,
        policy: SwitchPolicy,
        local: String,
        local_monitors: Vec<Rect>,
        positions: BTreeMap<String, Point>,
    ) -> Self {
        let mut core = Self {
            layout,
            policy,
            local,
            local_monitors,
            positions,
            sizes: BTreeMap::new(),
            origins: HashMap::new(),
            connected: HashMap::new(),
            state: State::Local,
            last_local_monitor: None,
            switch: SwitchState::new(),
            seq: 0,
        };
        core.refresh_origins();
        core
    }

    /// Start with these desk sizes instead of every machine's real one.
    pub fn with_sizes(mut self, sizes: BTreeMap<String, Size>) -> Self {
        self.sizes = sizes;
        self.refresh_origins();
        self
    }

    /// The remote screen currently holding the input, if any.
    pub fn active_remote(&self) -> Option<&str> {
        match &self.state {
            State::Local => None,
            State::Remote { screen, .. } => Some(screen),
        }
    }

    pub fn client_connected(&mut self, name: &str, monitors: Vec<Rect>) {
        self.connected.insert(name.to_string(), monitors);
        self.refresh_origins();
    }

    /// A machine the config did not list was approved and joined the desk. With no
    /// position of its own it lines up beside the screens already there.
    pub fn add_screen(&mut self, name: &str) {
        if self.layout.add_screen(name) {
            self.refresh_origins();
        }
    }

    /// This machine's own monitors changed while the server runs. Everything that
    /// was worked out from the old list (where the screens sit on the desk, which
    /// edge touches a client, where input comes back to) follows the new one.
    pub fn set_local_monitors(&mut self, monitors: Vec<Rect>) {
        if monitors.is_empty() || monitors == self.local_monitors {
            return;
        }
        self.local_monitors = monitors;
        if self
            .last_local_monitor
            .is_some_and(|m| !self.local_monitors.contains(&m))
        {
            self.last_local_monitor = None; // that monitor is gone
        }
        self.refresh_origins();
    }

    /// The layout editor moved machines.
    pub fn set_positions(&mut self, positions: BTreeMap<String, Point>) {
        self.positions = positions;
        self.refresh_origins();
    }

    /// The layout editor stretched or reset machines.
    pub fn set_sizes(&mut self, sizes: BTreeMap<String, Size>) {
        self.sizes = sizes;
        self.refresh_origins();
    }

    /// A client went away. If it held the input, take it back.
    pub fn client_disconnected(&mut self, name: &str) -> Vec<Action> {
        self.connected.remove(name);
        self.refresh_origins();
        match &self.state {
            State::Remote { screen, .. } if screen == name => {
                info!(
                    screen = name,
                    "active screen disconnected; input returns to local"
                );
                self.state = State::Local;
                self.switch.switched();
                vec![Action::Capture(CaptureCmd::Ungrab {
                    warp: self.local_center(),
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

    // ---- desk geometry ------------------------------------------------------

    fn refresh_origins(&mut self) {
        let size_of = |name: &str| -> (i32, i32) {
            desk_size(self.monitors_of(name), self.sizes.get(name).copied())
        };
        self.origins = effective_origins(
            self.layout.screens(),
            &self.local,
            &self.layout,
            &self.positions,
            &size_of,
        );
    }

    fn origin(&self, name: &str) -> Point {
        self.origins.get(name).copied().unwrap_or(Point::new(0, 0))
    }

    /// How `name`'s own coordinates map onto the desk.
    fn frame(&self, name: &str) -> Frame {
        Frame::new(
            self.origin(name),
            Rect::union_all(self.monitors_of(name)),
            self.sizes.get(name).copied(),
        )
    }

    fn monitors_of(&self, name: &str) -> &[Rect] {
        if name == self.local {
            &self.local_monitors
        } else {
            self.connected.get(name).map_or(&[], Vec::as_slice)
        }
    }

    /// Which machine (other than `exclude`) has a monitor at desk point `v`,
    /// and where that is in the machine's own coordinates.
    fn locate(&self, v: Point, exclude: &str) -> Option<(String, Point)> {
        let mut names: Vec<&String> = self.connected.keys().collect();
        names.push(&self.local);
        names.sort();
        names
            .into_iter()
            .filter(|n| n.as_str() != exclude)
            .find_map(|n| {
                let q = self.frame(n).to_local(v);
                self.monitors_of(n)
                    .iter()
                    .any(|m| m.contains(q))
                    .then(|| (n.clone(), q))
            })
    }

    /// Where input returns to on the local screen when nothing says where: the
    /// middle of the monitor the pointer was last on, else of the primary one (the
    /// one at the origin). Not the middle of the whole desktop, which for monitors
    /// stacked or staggered can lie on a seam, or on no monitor at all.
    fn local_center(&self) -> Point {
        let home = self
            .last_local_monitor
            .filter(|m| self.local_monitors.contains(m))
            .or_else(|| {
                self.local_monitors
                    .iter()
                    .copied()
                    .find(|m| m.contains(Point::new(0, 0)))
            })
            .or_else(|| self.local_monitors.first().copied());
        let c = home.map_or(Point::new(0, 0), |m| m.center());
        self.snap_into(&self.local.clone(), c)
    }

    /// `p` moved onto the nearest monitor of `name`.
    fn snap_into(&self, name: &str, p: Point) -> Point {
        let mons = self.monitors_of(name);
        if mons.is_empty() {
            p
        } else {
            nearest_point(p, mons)
        }
    }

    // ---- local mode ---------------------------------------------------------

    fn on_pointer(&mut self, p: Point, now_ms: u64) -> Vec<Action> {
        if self.state != State::Local {
            return Vec::new(); // stale sample from before the grab
        }
        let Some(m) = self.local_monitors.iter().copied().find(|m| m.contains(p)) else {
            return Vec::new();
        };
        self.last_local_monitor = Some(m);
        let jump = self.policy.jump_zone.max(1);
        let local_frame = self.frame(&self.local);

        let mut hit = None;
        for edge in Edge::ALL {
            if !at_edge(&m, p, edge, jump) {
                continue;
            }
            let probe = step(p, edge, jump);
            if self.local_monitors.iter().any(|x| x.contains(probe)) {
                continue; // a seam between two of our own monitors
            }
            let desk = local_frame.to_desk(probe);
            if let Some((name, q)) = self.locate(desk, &self.local) {
                hit = Some((edge, name, q));
                break;
            }
        }
        let Some((edge, target, q)) = hit else {
            self.switch.left_edge();
            return Vec::new();
        };

        let in_corner = near_corner(&m, p, edge, self.policy.corner_size);
        match self
            .switch
            .evaluate(&self.policy, now_ms, edge, true, in_corner)
        {
            Decision::Switch => self.enter(target, q),
            Decision::Wait | Decision::Blocked => Vec::new(),
        }
    }

    fn on_hotkey(&mut self) -> Vec<Action> {
        match &self.state {
            State::Remote { screen, .. } => {
                let screen = screen.clone();
                let at = self.local_center();
                self.go_local(&screen, at)
            }
            State::Local => {
                // The nearest connected machine on the desk.
                let lc = self.frame(&self.local).to_desk(
                    Rect::union_all(&self.local_monitors).map_or(Point::new(0, 0), |r| r.center()),
                );
                let target = self
                    .connected
                    .iter()
                    .filter_map(|(n, mons)| {
                        let c = self.frame(n).to_desk(Rect::union_all(mons)?.center());
                        let (dx, dy) = (i64::from(c.x - lc.x), i64::from(c.y - lc.y));
                        Some((dx * dx + dy * dy, n.clone()))
                    })
                    .min()
                    .map(|(_, n)| n);
                match target {
                    Some(t) => {
                        let c = Rect::union_all(&self.connected[&t])
                            .map_or(Point::new(0, 0), |r| r.center());
                        let at = self.snap_into(&t, c);
                        self.enter(t, at)
                    }
                    None => {
                        info!("hotkey pressed but no client is connected");
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
        let mons = self.monitors_of(&screen).to_vec();
        if mons.is_empty() {
            return Vec::new();
        }
        // The client may have re-arranged its monitors under the cursor.
        let pos = nearest_point(*pos, &mons);
        let moved = Point::new(pos.x + dx, pos.y + dy);

        let new_pos = if mons.iter().any(|m| m.contains(moved)) {
            moved
        } else {
            // Leaving this machine's monitors: is another machine's monitor
            // there on the desk?
            let desk = self.frame(&screen).to_desk(moved);
            if let Some((next, q)) = self.locate(desk, &screen) {
                // The move that crosses the edge is never sent as a position,
                // so after a fast flick the client's cursor would be left
                // wherever it last was. Put it on the edge it is leaving
                // through first.
                let edge = nearest_point(moved, &mons);
                let to_edge = Action::Send {
                    to: screen.clone(),
                    msg: ServerMsg::Mouse(MouseEvent::MoveAbs {
                        x: edge.x,
                        y: edge.y,
                    }),
                };
                if next == self.local {
                    let jump = self.policy.jump_zone.max(1);
                    let lm = self
                        .local_monitors
                        .iter()
                        .copied()
                        .find(|m| m.contains(q))
                        .unwrap_or(self.local_monitors[0]);
                    let mut actions = vec![to_edge];
                    actions.extend(self.go_local(&screen, inset_clamp(q, &lm, jump + 1)));
                    return actions;
                }
                let mut actions = vec![
                    to_edge,
                    Action::Send {
                        to: screen,
                        msg: ServerMsg::Leave,
                    },
                ];
                actions.extend(self.enter_from_remote(next, q));
                return actions;
            }
            nearest_point(moved, &mons) // nothing beyond: stop at the edge
        };

        self.state = State::Remote {
            screen: screen.clone(),
            pos: new_pos,
        };
        vec![Action::Send {
            to: screen,
            msg: ServerMsg::Mouse(MouseEvent::MoveAbs {
                x: new_pos.x,
                y: new_pos.y,
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

/// `p` moved `n` pixels towards `edge`.
fn step(p: Point, edge: Edge, n: i32) -> Point {
    match edge {
        Edge::Left => Point::new(p.x - n, p.y),
        Edge::Right => Point::new(p.x + n, p.y),
        Edge::Top => Point::new(p.x, p.y - n),
        Edge::Bottom => Point::new(p.x, p.y + n),
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

/// Drive [`Core`] from the capture backend and the client hub until either
/// input stream ends.
pub async fn run(
    core: Core,
    hub: Arc<Hub>,
    capture: CaptureHandle,
    capture_events: UnboundedReceiver<CaptureEvent>,
    hub_events: UnboundedReceiver<HubEvent>,
) {
    run_shared(core, hub, capture, capture_events, hub_events, None).await
}

/// [`run`], and when `files` is given, pulls a client's files onto this
/// server's own clipboard whenever the cursor returns here from a screen that
/// currently owns some.
pub async fn run_shared(
    mut core: Core,
    hub: Arc<Hub>,
    capture: CaptureHandle,
    mut capture_events: UnboundedReceiver<CaptureEvent>,
    mut hub_events: UnboundedReceiver<HubEvent>,
    files: Option<LocalFiles>,
) {
    let started = Instant::now();
    let mut last_pulled_seq: Option<u64> = None;
    loop {
        let was_remote = core.active_remote().is_some();
        let mut pending: VecDeque<Action> = tokio::select! {
            ev = capture_events.recv() => match ev {
                Some(ev) => core.handle(ev, started.elapsed().as_millis() as u64).into(),
                None => break,
            },
            ev = hub_events.recv() => match ev {
                Some(HubEvent::Connected { name, monitors }) => {
                    debug!(%name, monitors = monitors.len(), "client registered with the router");
                    core.client_connected(&name, monitors);
                    VecDeque::new()
                }
                Some(HubEvent::Positions(positions)) => {
                    info!(screens = positions.len(), "layout updated");
                    core.set_positions(positions);
                    VecDeque::new()
                }
                Some(HubEvent::Sizes(sizes)) => {
                    info!(stretched = sizes.len(), "desk sizes updated");
                    core.set_sizes(sizes);
                    VecDeque::new()
                }
                Some(HubEvent::ScreenAdded(name)) => {
                    info!(%name, "a new screen joined the desk");
                    core.add_screen(&name);
                    VecDeque::new()
                }
                Some(HubEvent::LocalMonitors(monitors)) => {
                    info!(monitors = monitors.len(), "this machine's monitors changed");
                    core.set_local_monitors(monitors);
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

        // The cursor just came back to this machine's own screen: if some
        // other screen currently owns files, pull them for a paste here.
        if was_remote && core.active_remote().is_none() {
            if let Some(lf) = files.as_ref().filter(|lf| lf.enabled) {
                if let Some((Owner::Client(name), seq, manifest)) = lf.router.current() {
                    if last_pulled_seq != Some(seq) {
                        last_pulled_seq = Some(seq);
                        files::spawn_local_pull(files::LocalPullJob {
                            router: Arc::clone(&lf.router),
                            sync: lf.sync.clone(),
                            peer: name,
                            seq,
                            manifest,
                            dest: files::staging_dir(seq),
                            files_max_bytes: lf.max_bytes,
                            hooks: lf.hooks.clone(),
                        });
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

    fn pos(entries: &[(&str, i32, i32)]) -> BTreeMap<String, Point> {
        entries
            .iter()
            .map(|(n, x, y)| (n.to_string(), Point::new(*x, *y)))
            .collect()
    }

    fn layout() -> Layout {
        Layout::new(vec!["linux-pc".into(), "win".into(), "mac".into()])
    }

    /// linux-pc at the origin, win directly to its right, mac right of win.
    fn row() -> BTreeMap<String, Point> {
        pos(&[("linux-pc", 0, 0), ("win", 1920, 0), ("mac", 4480, 0)])
    }

    fn core_with(policy: SwitchPolicy, positions: BTreeMap<String, Point>) -> Core {
        let mut c = Core::new(layout(), policy, "linux-pc".into(), vec![LOCAL], positions);
        c.client_connected("win", vec![REMOTE]);
        c
    }

    fn core() -> Core {
        core_with(SwitchPolicy::default(), row())
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
    fn pushing_the_right_edge_grabs_and_enters_at_the_same_height() {
        let mut c = core();
        let a = c.handle(CaptureEvent::Pointer(Point::new(1919, 540)), 0);
        assert_eq!(a[0], Action::Capture(CaptureCmd::Grab));
        // Same desk height, so the same y -- no proportional scaling.
        assert!(matches!(
            sends(&a)[0],
            (
                "win",
                ServerMsg::Enter {
                    x: 0,
                    y: 540,
                    seq: 1,
                    ..
                }
            )
        ));
        assert_eq!(c.active_remote(), Some("win"));
    }

    #[test]
    fn a_vertical_offset_shifts_the_entry_point() {
        // win sits 300px lower than linux-pc.
        let mut c = core_with(
            SwitchPolicy::default(),
            pos(&[("linux-pc", 0, 0), ("win", 1920, 300)]),
        );
        let a = c.handle(CaptureEvent::Pointer(Point::new(1919, 500)), 0);
        assert!(matches!(
            sends(&a)[0],
            ("win", ServerMsg::Enter { x: 0, y: 200, .. })
        ));
    }

    /// Two server monitors stacked, and win (1920x1080) stretched to their height.
    fn stretched_core() -> Core {
        let mut c = Core::new(
            layout(),
            SwitchPolicy::default(),
            "linux-pc".into(),
            vec![LOCAL, Rect::new(0, 1080, 1920, 1080)],
            pos(&[("linux-pc", 0, 0), ("win", 1920, 0)]),
        )
        .with_sizes(BTreeMap::from([("win".to_string(), Size::new(1920, 2160))]));
        c.client_connected("win", vec![Rect::new(0, 0, 1920, 1080)]);
        c
    }

    #[test]
    fn a_stretched_client_is_reached_from_both_server_monitors_at_the_matching_height() {
        for (y, lands) in [(540, 270), (1620, 810), (1079, 539), (1080, 540)] {
            let mut c = stretched_core();
            let a = c.handle(CaptureEvent::Pointer(Point::new(1919, y)), 0);
            assert!(
                matches!(sends(&a)[0], ("win", ServerMsg::Enter { x: 0, y: ey, .. }) if *ey == lands),
                "server y {y}: {a:?}"
            );
        }
    }

    #[test]
    fn leaving_a_stretched_client_lands_where_it_covers_on_the_desk() {
        let mut c = stretched_core();
        // In at server y 1620 (client y 810); push left out of the client.
        c.handle(CaptureEvent::Pointer(Point::new(1919, 1620)), 0);
        let a = c.handle(CaptureEvent::Motion { dx: -50, dy: 0 }, 1);
        assert_eq!(
            a.last().unwrap(),
            &Action::Capture(CaptureCmd::Ungrab {
                warp: Point::new(1920 - 50, 1620)
            })
        );
    }

    #[test]
    fn resetting_the_size_puts_the_client_back_to_its_real_extent() {
        let mut c = stretched_core();
        c.set_sizes(BTreeMap::new());
        // Below its real 1080px, the lower server monitor no longer touches it.
        assert!(c
            .handle(CaptureEvent::Pointer(Point::new(1919, 1620)), 0)
            .is_empty());
        assert!(!c
            .handle(CaptureEvent::Pointer(Point::new(1919, 540)), 1)
            .is_empty());
    }

    #[test]
    fn no_crossing_where_the_monitors_do_not_touch() {
        // win starts 300px lower, so the top 300px of the shared edge is dead.
        let mut c = core_with(
            SwitchPolicy::default(),
            pos(&[("linux-pc", 0, 0), ("win", 1920, 300)]),
        );
        assert!(c
            .handle(CaptureEvent::Pointer(Point::new(1919, 100)), 0)
            .is_empty());
        assert!(!c
            .handle(CaptureEvent::Pointer(Point::new(1919, 300)), 1)
            .is_empty());
    }

    #[test]
    fn a_gap_between_machines_blocks_crossing() {
        let mut c = core_with(
            SwitchPolicy::default(),
            pos(&[("linux-pc", 0, 0), ("win", 2100, 0)]),
        );
        assert!(c
            .handle(CaptureEvent::Pointer(Point::new(1919, 500)), 0)
            .is_empty());
    }

    #[test]
    fn crossing_works_in_every_direction() {
        for (win_at, ptr) in [
            ((-2560, 0), Point::new(0, 500)),   // win on the left
            ((0, -1440), Point::new(500, 0)),   // above
            ((0, 1080), Point::new(500, 1079)), // below
        ] {
            let mut c = core_with(
                SwitchPolicy::default(),
                pos(&[("linux-pc", 0, 0), ("win", win_at.0, win_at.1)]),
            );
            let a = c.handle(CaptureEvent::Pointer(ptr), 0);
            assert!(!a.is_empty(), "no crossing for win at {win_at:?}");
            assert_eq!(c.active_remote(), Some("win"));
        }
    }

    #[test]
    fn leaving_a_monitor_for_another_of_the_same_machine_is_not_a_crossing() {
        let mut c = Core::new(
            layout(),
            SwitchPolicy::default(),
            "linux-pc".into(),
            vec![Rect::new(0, 0, 1000, 1000), Rect::new(1000, 0, 1000, 1000)],
            pos(&[("linux-pc", 0, 0), ("win", 2000, 0)]),
        );
        c.client_connected("win", vec![REMOTE]);
        // The seam between our two monitors:
        assert!(c
            .handle(CaptureEvent::Pointer(Point::new(999, 500)), 0)
            .is_empty());
        // The real outer edge:
        assert!(!c
            .handle(CaptureEvent::Pointer(Point::new(1999, 500)), 1)
            .is_empty());
    }

    #[test]
    fn edge_without_a_connected_neighbour_is_ignored() {
        let mut c = Core::new(
            layout(),
            SwitchPolicy::default(),
            "linux-pc".into(),
            vec![LOCAL],
            row(),
        );
        assert!(c
            .handle(CaptureEvent::Pointer(Point::new(1919, 10)), 0)
            .is_empty());
        c.client_connected("win", vec![REMOTE]);
        assert!(c
            .handle(CaptureEvent::Pointer(Point::new(0, 10)), 0)
            .is_empty());
    }

    #[test]
    fn legacy_links_still_place_screens() {
        let mut l = layout();
        l.link("linux-pc", Edge::Right, "win").unwrap();
        l.link("win", Edge::Left, "linux-pc").unwrap();
        let mut c = Core::new(
            l,
            SwitchPolicy::default(),
            "linux-pc".into(),
            vec![LOCAL],
            BTreeMap::new(),
        );
        c.client_connected("win", vec![REMOTE]);
        let a = c.handle(CaptureEvent::Pointer(Point::new(1919, 540)), 0);
        assert!(matches!(
            sends(&a)[0],
            ("win", ServerMsg::Enter { x: 0, y: 540, .. })
        ));
    }

    #[test]
    fn positions_can_be_changed_while_running() {
        let mut c = core();
        c.set_positions(pos(&[("linux-pc", 0, 0), ("win", 1920, 600)]));
        assert!(c
            .handle(CaptureEvent::Pointer(Point::new(1919, 100)), 0)
            .is_empty());
    }

    #[test]
    fn dwell_delay_holds_back_the_switch() {
        let mut c = core_with(
            SwitchPolicy {
                delay_ms: 200,
                ..SwitchPolicy::default()
            },
            row(),
        );
        let p = CaptureEvent::Pointer(Point::new(1919, 500));
        assert!(c.handle(p.clone(), 1000).is_empty());
        assert!(c.handle(p.clone(), 1100).is_empty());
        assert!(!c.handle(p, 1200).is_empty());
    }

    #[test]
    fn corner_constraint_limits_where_the_edge_switches() {
        let mut c = core_with(
            SwitchPolicy {
                corner_size: 100,
                ..SwitchPolicy::default()
            },
            row(),
        );
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
        assert!(matches!(
            sends(&a)[0],
            (
                "win",
                ServerMsg::Mouse(MouseEvent::MoveAbs { x: 30, y: 530 })
            )
        ));
    }

    #[test]
    fn cursor_stops_at_an_edge_with_no_neighbour() {
        let mut c = core();
        c.handle(CaptureEvent::Pointer(Point::new(1919, 540)), 0);
        let a = c.handle(CaptureEvent::Motion { dx: 0, dy: -5000 }, 1);
        assert!(matches!(
            sends(&a)[0],
            ("win", ServerMsg::Mouse(MouseEvent::MoveAbs { y: 0, .. }))
        ));
        assert_eq!(c.active_remote(), Some("win"));
    }

    #[test]
    fn pushing_back_through_the_shared_edge_returns_to_local_at_the_same_height() {
        let mut c = core();
        c.handle(CaptureEvent::Pointer(Point::new(1919, 540)), 0);
        // Barely past the edge: placed just inside so it does not re-trigger.
        let a = c.handle(CaptureEvent::Motion { dx: -1, dy: 0 }, 1);
        let s = sends(&a);
        assert!(matches!(
            s[0],
            ("win", ServerMsg::Mouse(MouseEvent::MoveAbs { x: 0, .. }))
        ));
        assert!(matches!(s[1], ("win", ServerMsg::Leave)));
        match a.last().unwrap() {
            Action::Capture(CaptureCmd::Ungrab { warp }) => {
                assert_eq!(*warp, Point::new(1920 - 1 - 2, 540));
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(c.active_remote(), None);
        assert!(c
            .handle(CaptureEvent::Pointer(Point::new(1917, 540)), 2)
            .is_empty());
    }

    #[test]
    fn a_fast_flick_out_leaves_the_client_cursor_on_the_edge_it_crossed() {
        let mut c = core();
        c.handle(CaptureEvent::Pointer(Point::new(1919, 540)), 0);
        // Move right into the client, well away from its left edge...
        c.handle(CaptureEvent::Motion { dx: 400, dy: 20 }, 1);
        // ...then flick back out in a single large step.
        let a = c.handle(CaptureEvent::Motion { dx: -900, dy: 30 }, 2);
        let s = sends(&a);
        // The client's cursor is first put on its left edge, at the height it
        // left at, and only then told the cursor has gone.
        assert!(matches!(
            s[0],
            (
                "win",
                ServerMsg::Mouse(MouseEvent::MoveAbs { x: 0, y: 590 })
            )
        ));
        assert!(matches!(s[1], ("win", ServerMsg::Leave)));
        assert_eq!(c.active_remote(), None);
    }

    #[test]
    fn a_hard_push_back_lands_where_the_cursor_would_naturally_be() {
        let mut c = core();
        c.handle(CaptureEvent::Pointer(Point::new(1919, 540)), 0);
        let a = c.handle(CaptureEvent::Motion { dx: -50, dy: 0 }, 1);
        assert_eq!(
            a.last().unwrap(),
            &Action::Capture(CaptureCmd::Ungrab {
                warp: Point::new(1920 - 50, 540)
            })
        );
    }

    #[test]
    fn returning_is_blocked_where_the_monitors_do_not_touch() {
        // win is 300px lower; at win's y=100 (desk y=400) we are still beside
        // linux-pc, but at desk y=1200 we are below it (linux-pc ends at 1080).
        let mut c = core_with(
            SwitchPolicy::default(),
            pos(&[("linux-pc", 0, 0), ("win", 1920, 300)]),
        );
        c.handle(CaptureEvent::Pointer(Point::new(1919, 900)), 0);
        // Enter at win y=600; go to win y=900 (desk 1200), then push left.
        c.handle(CaptureEvent::Motion { dx: 0, dy: 300 }, 1);
        let a = c.handle(CaptureEvent::Motion { dx: -50, dy: 0 }, 2);
        assert!(matches!(
            sends(&a)[0],
            ("win", ServerMsg::Mouse(MouseEvent::MoveAbs { x: 0, .. }))
        ));
        assert_eq!(c.active_remote(), Some("win"));
    }

    #[test]
    fn hopping_between_two_remote_screens_keeps_the_grab() {
        let mut c = core();
        c.client_connected("mac", vec![Rect::new(0, 0, 1440, 900)]);
        c.handle(CaptureEvent::Pointer(Point::new(1919, 540)), 0);
        // win spans desk x 1920..4480; mac starts at 4480.
        let a = c.handle(CaptureEvent::Motion { dx: 2600, dy: 0 }, 1);
        let s = sends(&a);
        assert!(matches!(
            s[0],
            ("win", ServerMsg::Mouse(MouseEvent::MoveAbs { x: 2559, .. }))
        ));
        assert!(matches!(s[1], ("win", ServerMsg::Leave)));
        assert!(matches!(
            s[2],
            ("mac", ServerMsg::Enter { x: 40, y: 540, .. })
        ));
        assert!(!a.iter().any(|x| matches!(x, Action::Capture(_))));
        assert_eq!(c.active_remote(), Some("mac"));
    }

    #[test]
    fn a_second_monitor_on_the_client_extends_its_reach() {
        let mut c = core();
        // win has two monitors side by side.
        c.client_connected(
            "win",
            vec![Rect::new(0, 0, 1000, 1000), Rect::new(1000, 0, 1000, 1000)],
        );
        c.handle(CaptureEvent::Pointer(Point::new(1919, 500)), 0);
        let a = c.handle(CaptureEvent::Motion { dx: 1500, dy: 0 }, 1);
        // Moved onto win's second monitor, no switch away.
        assert!(matches!(
            sends(&a)[0],
            (
                "win",
                ServerMsg::Mouse(MouseEvent::MoveAbs { x: 1500, y: 500 })
            )
        ));
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

    /// Two local monitors, one above the other: the middle of the whole desktop is
    /// the seam between them (and the old return point).
    fn stacked_core() -> Core {
        const ABOVE: Rect = Rect::new(0, -1080, 1920, 1080);
        let mut c = Core::new(
            layout(),
            SwitchPolicy::default(),
            "linux-pc".into(),
            vec![ABOVE, LOCAL],
            row(),
        );
        c.client_connected("win", vec![REMOTE]);
        c
    }

    fn returned_to(c: &mut Core) -> Point {
        c.handle(CaptureEvent::Hotkey, 1); // there
        match c.handle(CaptureEvent::Hotkey, 2).last() {
            Some(Action::Capture(CaptureCmd::Ungrab { warp })) => *warp,
            other => panic!("expected an Ungrab, got {other:?}"),
        }
    }

    #[test]
    fn the_hotkey_returns_to_the_middle_of_the_monitor_the_pointer_was_on() {
        let mut c = stacked_core();
        c.handle(CaptureEvent::Pointer(Point::new(700, -300)), 0); // the upper one
        assert_eq!(returned_to(&mut c), Point::new(960, -540));

        let mut c = stacked_core();
        c.handle(CaptureEvent::Pointer(Point::new(700, 300)), 0); // the lower one
        assert_eq!(returned_to(&mut c), Point::new(960, 540));
    }

    #[test]
    fn with_no_pointer_seen_it_returns_to_the_primary_monitor_not_the_seam() {
        let mut c = stacked_core();
        let back = returned_to(&mut c);
        assert_eq!(back, Point::new(960, 540), "the monitor at the origin");
        assert_ne!(back, Point::new(960, 0), "the seam between the two");
    }

    #[test]
    fn the_last_monitor_wins_when_the_pointer_moved_between_them() {
        let mut c = stacked_core();
        c.handle(CaptureEvent::Pointer(Point::new(700, -300)), 0);
        c.handle(CaptureEvent::Pointer(Point::new(700, 300)), 1);
        assert_eq!(returned_to(&mut c), Point::new(960, 540));
    }

    #[test]
    fn a_client_going_away_hands_the_input_back_to_the_same_place() {
        let mut c = stacked_core();
        c.handle(CaptureEvent::Pointer(Point::new(700, -300)), 0);
        c.handle(CaptureEvent::Hotkey, 1);
        let a = c.client_disconnected("win");
        assert_eq!(
            a.last().unwrap(),
            &Action::Capture(CaptureCmd::Ungrab {
                warp: Point::new(960, -540)
            })
        );
    }

    #[test]
    fn a_monitor_plugged_in_under_a_running_server_can_be_crossed_from() {
        // One local monitor, and win to the right of where a second would be.
        let mut c = Core::new(
            layout(),
            SwitchPolicy::default(),
            "linux-pc".into(),
            vec![LOCAL],
            pos(&[("linux-pc", 0, 0), ("win", 3840, 0)]),
        );
        c.client_connected("win", vec![REMOTE]);
        const NEW: Rect = Rect::new(1920, 0, 1920, 1080);

        // The pointer on a monitor the server does not know about crosses nothing.
        assert!(c
            .handle(CaptureEvent::Pointer(Point::new(3839, 540)), 0)
            .is_empty());

        c.set_local_monitors(vec![LOCAL, NEW]);
        let a = c.handle(CaptureEvent::Pointer(Point::new(3839, 540)), 1);
        assert_eq!(a[0], Action::Capture(CaptureCmd::Grab));
        assert!(matches!(
            sends(&a)[0],
            ("win", ServerMsg::Enter { x: 0, y: 540, .. })
        ));
    }

    #[test]
    fn a_screen_added_while_running_lines_up_beside_the_others_and_can_be_crossed_to() {
        let mut c = Core::new(
            Layout::new(vec!["linux-pc".into()]),
            SwitchPolicy::default(),
            "linux-pc".into(),
            vec![LOCAL],
            BTreeMap::new(),
        );
        c.add_screen("win");
        c.add_screen("win"); // approved twice changes nothing
        c.client_connected("win", vec![REMOTE]);

        // It sits just right of linux-pc (desk x 1920), so pushing the right edge
        // lands at its left edge, at the same height; at the origin (where a screen
        // with no place at all would fall) it would land 1920 pixels in.
        let a = c.handle(CaptureEvent::Pointer(Point::new(1919, 540)), 0);
        assert_eq!(a[0], Action::Capture(CaptureCmd::Grab));
        assert!(matches!(
            sends(&a)[0],
            ("win", ServerMsg::Enter { x: 0, y: 540, .. })
        ));
    }

    #[test]
    fn a_monitor_that_goes_away_is_forgotten_as_the_place_to_return_to() {
        let mut c = stacked_core();
        c.handle(CaptureEvent::Pointer(Point::new(700, -300)), 0); // on the upper one
        c.set_local_monitors(vec![LOCAL]); // which is then unplugged
        assert_eq!(returned_to(&mut c), Point::new(960, 540));
    }

    #[test]
    fn an_empty_or_unchanged_monitor_list_changes_nothing() {
        let mut c = stacked_core();
        c.handle(CaptureEvent::Pointer(Point::new(700, -300)), 0);
        c.set_local_monitors(Vec::new()); // the OS could not say
        c.set_local_monitors(vec![Rect::new(0, -1080, 1920, 1080), LOCAL]); // as before
        assert_eq!(
            returned_to(&mut c),
            Point::new(960, -540),
            "still the upper"
        );
    }

    #[test]
    fn hotkey_goes_to_the_nearest_machine() {
        let mut c = core();
        c.client_connected("mac", vec![REMOTE]);
        let a = c.handle(CaptureEvent::Hotkey, 0);
        assert!(matches!(sends(&a)[0], ("win", _)));
    }

    #[test]
    fn hotkey_with_nobody_connected_does_nothing() {
        let mut c = Core::new(
            layout(),
            SwitchPolicy::default(),
            "linux-pc".into(),
            vec![LOCAL],
            row(),
        );
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
        c.client_connected("win", vec![REMOTE]);
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
            x = 0
            y = 0
            [[screens]]
            name = "win"
            x = 1920
            y = 0
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
            vec![LOCAL],
            config.positions.clone(),
        );
        tokio::spawn(run(core, Arc::clone(&hub), capture, cap_rx, hub_events));

        let (_handle, listener) = crate::net::bind(&config).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (_, never) = tokio::sync::watch::channel(false);
        tokio::spawn(accept_loop_with(config, listener, hub, never));

        let mut ms = MessageStream::new(TcpStream::connect(addr).await.unwrap());
        ms.send(&ClientMsg::Hello {
            version: PROTOCOL_VERSION,
            screen: ScreenInfo::from_monitors("win", vec![REMOTE], Point::new(0, 0)),
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
                ServerMsg::Enter { x: 0, y: 540, .. } => saw.0 = true,
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
