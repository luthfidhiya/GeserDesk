// SPDX-License-Identifier: GPL-3.0-or-later
//! The shared virtual desk: where each machine's monitors sit relative to the
//! others.
//!
//! Every machine keeps its own coordinate space (its primary monitor at the
//! origin). The desk gives each machine an *origin* in one common space; a
//! monitor's place on the desk is its local rectangle shifted by that origin.
//! Two monitors that touch on the desk are neighbours, which is how offsets
//! (one screen sitting lower than the next) work without any per-edge
//! configuration.

use std::collections::{BTreeMap, HashMap, VecDeque};

use geserdesk_proto::{Edge, Point, Rect};

use crate::layout::Layout;

/// Assumed size of a machine whose monitors are not known yet.
pub const DEFAULT_SIZE: (i32, i32) = (1920, 1080);

/// Work out an origin for every screen in `order`.
///
/// 1. Screens with a `configured` origin keep it.
/// 2. Others are placed beside an already-placed neighbour following the
///    `[links]` in `layout` (so configs written before positions existed keep
///    working).
/// 3. Any still unplaced go in a row to the right of everything else.
///
/// `size_of` gives a machine's `(width, height)`.
pub fn effective_origins(
    order: &[String],
    local: &str,
    layout: &Layout,
    configured: &BTreeMap<String, Point>,
    size_of: &dyn Fn(&str) -> (i32, i32),
) -> HashMap<String, Point> {
    let mut placed: HashMap<String, Point> = configured
        .iter()
        .filter(|(name, _)| order.contains(name))
        .map(|(n, p)| (n.clone(), *p))
        .collect();
    placed.entry(local.to_string()).or_insert(Point::new(0, 0));

    // Follow links outwards from everything placed so far.
    let mut queue: VecDeque<String> = order
        .iter()
        .filter(|n| placed.contains_key(*n))
        .cloned()
        .collect();
    while let Some(a) = queue.pop_front() {
        let oa = placed[&a];
        let (wa, ha) = size_of(&a);
        for edge in Edge::ALL {
            let Some(b) = layout.neighbor(&a, edge) else {
                continue;
            };
            if placed.contains_key(b) || !order.iter().any(|n| n == b) {
                continue;
            }
            let (wb, hb) = size_of(b);
            let origin = match edge {
                Edge::Right => Point::new(oa.x + wa, oa.y),
                Edge::Left => Point::new(oa.x - wb, oa.y),
                Edge::Bottom => Point::new(oa.x, oa.y + ha),
                Edge::Top => Point::new(oa.x, oa.y - hb),
            };
            placed.insert(b.to_string(), origin);
            queue.push_back(b.to_string());
        }
    }

    // Whatever is left goes to the right, in config order.
    let mut right = order
        .iter()
        .filter_map(|n| placed.get(n).map(|o| o.x + size_of(n).0))
        .max()
        .unwrap_or(0);
    for name in order {
        if !placed.contains_key(name) {
            placed.insert(name.clone(), Point::new(right, 0));
            right += size_of(name).0;
        }
    }
    placed
}

/// The point inside any of `monitors` closest to `p` (or `p` itself if it is
/// already inside one). `monitors` must not be empty.
pub fn nearest_point(p: Point, monitors: &[Rect]) -> Point {
    monitors
        .iter()
        .map(|m| m.clamp(p))
        .min_by_key(|c| {
            let (dx, dy) = (i64::from(c.x - p.x), i64::from(c.y - p.y));
            dx * dx + dy * dy
        })
        .unwrap_or(p)
}

/// Clamp `p` into `r`, staying `inset` pixels away from every side where the
/// rectangle is big enough; otherwise the centre.
pub fn inset_clamp(p: Point, r: &Rect, inset: i32) -> Point {
    let axis = |v: i32, lo: i32, len: i32, centre: i32| {
        if len > 2 * inset + 1 {
            v.clamp(lo + inset, lo + len - 1 - inset)
        } else {
            centre
        }
    };
    let c = r.center();
    Point::new(axis(p.x, r.x, r.w, c.x), axis(p.y, r.y, r.h, c.y))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn size(name: &str) -> (i32, i32) {
        match name {
            "win" => (2560, 1440),
            _ => (1920, 1080),
        }
    }

    #[test]
    fn configured_positions_win() {
        let layout = Layout::new(names(&["a", "b"]));
        let mut cfg = BTreeMap::new();
        cfg.insert("b".to_string(), Point::new(-2560, 300));
        let o = effective_origins(&names(&["a", "b"]), "a", &layout, &cfg, &size);
        assert_eq!(o["a"], Point::new(0, 0));
        assert_eq!(o["b"], Point::new(-2560, 300));
    }

    #[test]
    fn links_place_unpositioned_screens() {
        let mut layout = Layout::new(names(&["a", "win", "c"]));
        layout.link("a", Edge::Right, "win").unwrap();
        layout.link("win", Edge::Bottom, "c").unwrap();
        let o = effective_origins(
            &names(&["a", "win", "c"]),
            "a",
            &layout,
            &BTreeMap::new(),
            &size,
        );
        assert_eq!(o["win"], Point::new(1920, 0));
        assert_eq!(o["c"], Point::new(1920, 1440));
    }

    #[test]
    fn left_link_uses_the_neighbours_width() {
        let mut layout = Layout::new(names(&["a", "win"]));
        layout.link("a", Edge::Left, "win").unwrap();
        let o = effective_origins(&names(&["a", "win"]), "a", &layout, &BTreeMap::new(), &size);
        assert_eq!(o["win"], Point::new(-2560, 0));
    }

    #[test]
    fn unlinked_screens_line_up_to_the_right() {
        let layout = Layout::new(names(&["a", "b", "c"]));
        let o = effective_origins(
            &names(&["a", "b", "c"]),
            "a",
            &layout,
            &BTreeMap::new(),
            &size,
        );
        assert_eq!(o["b"], Point::new(1920, 0));
        assert_eq!(o["c"], Point::new(3840, 0));
    }

    #[test]
    fn nearest_point_picks_the_closest_monitor() {
        let m = [Rect::new(0, 0, 100, 100), Rect::new(200, 0, 100, 100)];
        assert_eq!(nearest_point(Point::new(50, 50), &m), Point::new(50, 50));
        assert_eq!(nearest_point(Point::new(180, 50), &m), Point::new(200, 50));
        assert_eq!(nearest_point(Point::new(120, -40), &m), Point::new(99, 0));
    }

    #[test]
    fn inset_clamp_keeps_away_from_the_sides() {
        let r = Rect::new(0, 0, 100, 100);
        assert_eq!(inset_clamp(Point::new(99, 50), &r, 3), Point::new(96, 50));
        assert_eq!(inset_clamp(Point::new(-9, -9), &r, 3), Point::new(3, 3));
        assert_eq!(
            inset_clamp(Point::new(1, 1), &Rect::new(0, 0, 5, 5), 3),
            Point::new(2, 2)
        );
    }
}
