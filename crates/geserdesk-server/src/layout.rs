// SPDX-License-Identifier: GPL-3.0-or-later
//! Pure screen-layout logic: adjacency and switch policy.
//!
//! Ported *in concept* from Input Leap's `Server.cpp` (`mapToNeighbor`,
//! `isSwitchOkay`, `getJumpZoneSize`, `isLockedToScreen`) -- rewritten, not
//! transliterated. Everything here is free of I/O and of wall-clock time (the
//! caller passes a monotonic millisecond stamp), so it is fully unit-testable
//! without any devices.

use std::collections::BTreeMap;

use geserdesk_proto::{Edge, EdgeSet};

/// The static arrangement of screens: names plus per-edge neighbours.
#[derive(Debug, Clone)]
pub struct Layout {
    screens: Vec<String>,
    /// `links[from][edge] = to`
    links: BTreeMap<String, BTreeMap<Edge, String>>,
}

/// A link named a screen not present in `[[screens]]`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("link {from:?} -> {to:?} references undefined screen {undefined:?}")]
pub struct LinkError {
    pub from: String,
    pub to: String,
    pub undefined: String,
}

impl Layout {
    pub fn new(screens: Vec<String>) -> Self {
        Self {
            screens,
            links: BTreeMap::new(),
        }
    }

    pub fn screens(&self) -> &[String] {
        &self.screens
    }

    pub fn has_screen(&self, name: &str) -> bool {
        self.screens.iter().any(|s| s == name)
    }

    /// Record that crossing `edge` of `from` lands on `to`.
    pub fn link(&mut self, from: &str, edge: Edge, to: &str) -> Result<(), LinkError> {
        for name in [from, to] {
            if !self.has_screen(name) {
                return Err(LinkError {
                    from: from.to_string(),
                    to: to.to_string(),
                    undefined: name.to_string(),
                });
            }
        }
        self.links
            .entry(from.to_string())
            .or_default()
            .insert(edge, to.to_string());
        Ok(())
    }

    /// The screen reached by leaving `from` across `edge`, if any.
    ///
    /// This is the equivalent of Input Leap's `Server::mapToNeighbor`.
    pub fn neighbor(&self, from: &str, edge: Edge) -> Option<&str> {
        self.links.get(from)?.get(&edge).map(String::as_str)
    }

    /// The set of edges of `from` that have a neighbour. The capture backend
    /// watches exactly these for a cursor crossing.
    pub fn active_edges(&self, from: &str) -> EdgeSet {
        let Some(m) = self.links.get(from) else {
            return EdgeSet::empty();
        };
        m.keys().copied().collect()
    }
}

/// Tunables governing whether a cursor at a screen edge actually switches
/// screens. Mirrors the `kOptionScreenSwitch*` family in Input Leap's
/// `option_types.h`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SwitchPolicy {
    /// Cursor must rest against the edge for this many ms before switching.
    /// `0` disables the dwell requirement.
    pub delay_ms: u32,
    /// Cursor must touch the edge twice within this many ms to switch.
    /// `0` disables the double-tap requirement.
    pub double_tap_ms: u32,
    /// Only switch within this many px of a screen corner. `0` = whole edge.
    pub corner_size: i32,
    /// Width of the edge band that counts as "at the edge" (Input Leap's jump
    /// zone). At least 1.
    pub jump_zone: i32,
    /// Whether an engaged Scroll Lock pins the cursor to the active screen.
    pub scroll_lock_locks: bool,
}

impl Default for SwitchPolicy {
    fn default() -> Self {
        Self {
            delay_ms: 0,
            double_tap_ms: 0,
            corner_size: 0,
            jump_zone: 1,
            scroll_lock_locks: true,
        }
    }
}

/// The decision produced by [`SwitchState::evaluate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Switch now, to the neighbour across `edge`.
    Switch,
    /// The cursor is at a live edge but a gate (dwell / second tap) is not yet
    /// satisfied. Keep feeding edge samples.
    Wait,
    /// No switch: no neighbour, locked to screen, or outside the corner window.
    Blocked,
}

/// Mutable state the caller threads between [`evaluate`](Self::evaluate) calls.
#[derive(Debug, Clone, Default)]
pub struct SwitchState {
    /// Scroll Lock engaged.
    locked: bool,
    /// A dwell in progress: `(edge, started_at_ms)`.
    dwell: Option<(Edge, u64)>,
    /// Last edge tap seen: `(edge, at_ms)`, for double-tap detection.
    last_tap: Option<(Edge, u64)>,
}

impl SwitchState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_locked(&self) -> bool {
        self.locked
    }

    /// Flip the Scroll Lock state (called when the server sees the key toggle).
    pub fn toggle_lock(&mut self) {
        self.locked = !self.locked;
        self.dwell = None;
    }

    pub fn set_locked(&mut self, locked: bool) {
        self.locked = locked;
        if locked {
            self.dwell = None;
        }
    }

    /// Call when the cursor leaves the edge band without switching, so a stale
    /// dwell doesn't fire later.
    pub fn left_edge(&mut self) {
        self.dwell = None;
    }

    /// Call after a switch actually happens, to reset per-attempt state.
    pub fn switched(&mut self) {
        self.dwell = None;
        self.last_tap = None;
    }

    /// Decide whether the cursor, currently in the `edge` jump zone with
    /// `has_neighbor` telling whether a link exists and `in_corner` telling
    /// whether corner constraints are met, should switch at monotonic time
    /// `now_ms`.
    pub fn evaluate(
        &mut self,
        policy: &SwitchPolicy,
        now_ms: u64,
        edge: Edge,
        has_neighbor: bool,
        in_corner: bool,
    ) -> Decision {
        if !has_neighbor || !in_corner {
            self.dwell = None;
            return Decision::Blocked;
        }
        if self.locked && policy.scroll_lock_locks {
            self.dwell = None;
            return Decision::Blocked;
        }

        // Gate 1: double-tap (takes precedence when enabled).
        if policy.double_tap_ms > 0 {
            let second_tap = matches!(
                self.last_tap,
                Some((e, t)) if e == edge && now_ms.saturating_sub(t) <= policy.double_tap_ms as u64
            );
            self.last_tap = Some((edge, now_ms));
            if !second_tap {
                return Decision::Wait;
            }
        }

        // Gate 2: dwell delay.
        if policy.delay_ms > 0 {
            match self.dwell {
                Some((e, started)) if e == edge => {
                    if now_ms.saturating_sub(started) >= policy.delay_ms as u64 {
                        self.switched();
                        return Decision::Switch;
                    }
                    return Decision::Wait;
                }
                _ => {
                    self.dwell = Some((edge, now_ms));
                    return Decision::Wait;
                }
            }
        }

        self.switched();
        Decision::Switch
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> Layout {
        let mut l = Layout::new(vec!["a".into(), "b".into(), "c".into()]);
        l.link("a", Edge::Right, "b").unwrap();
        l.link("b", Edge::Left, "a").unwrap();
        l.link("b", Edge::Right, "c").unwrap();
        l
    }

    #[test]
    fn neighbor_lookup() {
        let l = layout();
        assert_eq!(l.neighbor("a", Edge::Right), Some("b"));
        assert_eq!(l.neighbor("b", Edge::Right), Some("c"));
        assert_eq!(l.neighbor("a", Edge::Left), None);
        assert_eq!(l.neighbor("nope", Edge::Left), None);
    }

    #[test]
    fn active_edges_reports_linked_sides() {
        let l = layout();
        let e = l.active_edges("b");
        assert!(e.contains(Edge::Left));
        assert!(e.contains(Edge::Right));
        assert!(!e.contains(Edge::Top));
        assert!(l.active_edges("c").is_empty());
    }

    #[test]
    fn link_to_unknown_screen_errs() {
        let mut l = Layout::new(vec!["a".into()]);
        let err = l.link("a", Edge::Right, "ghost").unwrap_err();
        assert_eq!(err.undefined, "ghost");
    }

    #[test]
    fn immediate_switch_when_no_gates() {
        let p = SwitchPolicy::default();
        let mut s = SwitchState::new();
        assert_eq!(s.evaluate(&p, 0, Edge::Right, true, true), Decision::Switch);
    }

    #[test]
    fn blocked_without_neighbor_or_corner() {
        let p = SwitchPolicy::default();
        let mut s = SwitchState::new();
        assert_eq!(
            s.evaluate(&p, 0, Edge::Right, false, true),
            Decision::Blocked
        );
        assert_eq!(
            s.evaluate(&p, 0, Edge::Right, true, false),
            Decision::Blocked
        );
    }

    #[test]
    fn scroll_lock_blocks_switch() {
        let p = SwitchPolicy::default();
        let mut s = SwitchState::new();
        s.toggle_lock();
        assert!(s.is_locked());
        assert_eq!(
            s.evaluate(&p, 0, Edge::Right, true, true),
            Decision::Blocked
        );
        s.toggle_lock();
        assert_eq!(s.evaluate(&p, 0, Edge::Right, true, true), Decision::Switch);
    }

    #[test]
    fn dwell_delay_requires_time_at_edge() {
        let p = SwitchPolicy {
            delay_ms: 200,
            ..SwitchPolicy::default()
        };
        let mut s = SwitchState::new();
        assert_eq!(
            s.evaluate(&p, 1000, Edge::Right, true, true),
            Decision::Wait
        );
        assert_eq!(
            s.evaluate(&p, 1100, Edge::Right, true, true),
            Decision::Wait
        );
        assert_eq!(
            s.evaluate(&p, 1200, Edge::Right, true, true),
            Decision::Switch
        );
    }

    #[test]
    fn dwell_resets_when_switching_edges() {
        let p = SwitchPolicy {
            delay_ms: 200,
            ..SwitchPolicy::default()
        };
        let mut s = SwitchState::new();
        s.evaluate(&p, 1000, Edge::Right, true, true);
        // Different edge -> dwell restarts.
        assert_eq!(s.evaluate(&p, 1300, Edge::Left, true, true), Decision::Wait);
    }

    #[test]
    fn double_tap_requires_two_touches_in_window() {
        let p = SwitchPolicy {
            double_tap_ms: 250,
            ..SwitchPolicy::default()
        };
        let mut s = SwitchState::new();
        // First touch: wait.
        assert_eq!(
            s.evaluate(&p, 1000, Edge::Right, true, true),
            Decision::Wait
        );
        // Second touch within window: switch.
        assert_eq!(
            s.evaluate(&p, 1200, Edge::Right, true, true),
            Decision::Switch
        );
    }

    #[test]
    fn double_tap_too_slow_does_not_switch() {
        let p = SwitchPolicy {
            double_tap_ms: 250,
            ..SwitchPolicy::default()
        };
        let mut s = SwitchState::new();
        assert_eq!(
            s.evaluate(&p, 1000, Edge::Right, true, true),
            Decision::Wait
        );
        assert_eq!(
            s.evaluate(&p, 2000, Edge::Right, true, true),
            Decision::Wait
        );
        // ...but the 2s touch is now itself a first tap; a quick follow-up works.
        assert_eq!(
            s.evaluate(&p, 2100, Edge::Right, true, true),
            Decision::Switch
        );
    }

    #[test]
    fn left_edge_clears_pending_dwell() {
        let p = SwitchPolicy {
            delay_ms: 200,
            ..SwitchPolicy::default()
        };
        let mut s = SwitchState::new();
        s.evaluate(&p, 1000, Edge::Right, true, true);
        s.left_edge();
        // Dwell restarts rather than firing immediately at t=1200.
        assert_eq!(
            s.evaluate(&p, 1200, Edge::Right, true, true),
            Decision::Wait
        );
    }
}
