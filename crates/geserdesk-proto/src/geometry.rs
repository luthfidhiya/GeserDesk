// SPDX-License-Identifier: GPL-3.0-or-later
//! Screen geometry: points, rectangles, and the four edges a cursor can cross.

use serde::{Deserialize, Serialize};

/// A point in absolute screen coordinates. `i32` so ultra-wide multi-monitor
/// layouts are representable (Input Leap's `i16` capped this at 32767).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

impl Point {
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}

/// An axis-aligned rectangle, `[x, x+w)` by `[y, y+h)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    pub const fn left(&self) -> i32 {
        self.x
    }
    pub const fn top(&self) -> i32 {
        self.y
    }
    pub const fn right(&self) -> i32 {
        self.x + self.w
    }
    pub const fn bottom(&self) -> i32 {
        self.y + self.h
    }
    pub const fn center(&self) -> Point {
        Point::new(self.x + self.w / 2, self.y + self.h / 2)
    }

    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.left() && p.x < self.right() && p.y >= self.top() && p.y < self.bottom()
    }

    /// Clamp a point to lie within the rectangle (inclusive of the far edge
    /// minus one pixel).
    pub fn clamp(&self, p: Point) -> Point {
        Point::new(
            p.x.clamp(self.left(), self.right() - 1),
            p.y.clamp(self.top(), self.bottom() - 1),
        )
    }
}

/// One side of a screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Edge {
    Left,
    Right,
    Top,
    Bottom,
}

impl Edge {
    pub const ALL: [Edge; 4] = [Edge::Left, Edge::Right, Edge::Top, Edge::Bottom];

    pub fn opposite(self) -> Edge {
        match self {
            Edge::Left => Edge::Right,
            Edge::Right => Edge::Left,
            Edge::Top => Edge::Bottom,
            Edge::Bottom => Edge::Top,
        }
    }
}

/// A set of edges, e.g. the sides of the primary screen that currently have a
/// neighbour and should be watched for a cursor crossing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EdgeSet(u8);

impl EdgeSet {
    const fn bit(edge: Edge) -> u8 {
        1 << (edge as u8)
    }

    pub const fn empty() -> Self {
        EdgeSet(0)
    }

    pub fn with(mut self, edge: Edge) -> Self {
        self.0 |= Self::bit(edge);
        self
    }

    pub fn insert(&mut self, edge: Edge) {
        self.0 |= Self::bit(edge);
    }

    pub fn contains(&self, edge: Edge) -> bool {
        self.0 & Self::bit(edge) != 0
    }

    pub fn is_empty(&self) -> bool {
        self.0 == 0
    }
}

impl FromIterator<Edge> for EdgeSet {
    fn from_iter<T: IntoIterator<Item = Edge>>(iter: T) -> Self {
        iter.into_iter().fold(EdgeSet::empty(), EdgeSet::with)
    }
}

/// A screen's identity and geometry, sent by the client at connect time and
/// whenever its resolution changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenInfo {
    /// Logical name, must match the server config.
    pub name: String,
    /// Full desktop bounds (union of all monitors on that machine).
    pub bounds: Rect,
    /// Cursor position at the time the info was captured.
    pub cursor: Point,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_edges_and_center() {
        let r = Rect::new(10, 20, 100, 200);
        assert_eq!(r.right(), 110);
        assert_eq!(r.bottom(), 220);
        assert_eq!(r.center(), Point::new(60, 120));
    }

    #[test]
    fn rect_contains_is_half_open() {
        let r = Rect::new(0, 0, 10, 10);
        assert!(r.contains(Point::new(0, 0)));
        assert!(r.contains(Point::new(9, 9)));
        assert!(!r.contains(Point::new(10, 5)));
        assert!(!r.contains(Point::new(-1, 5)));
    }

    #[test]
    fn rect_clamp() {
        let r = Rect::new(0, 0, 100, 100);
        assert_eq!(r.clamp(Point::new(-5, 200)), Point::new(0, 99));
        assert_eq!(r.clamp(Point::new(50, 50)), Point::new(50, 50));
    }

    #[test]
    fn edge_set_roundtrip() {
        let s: EdgeSet = [Edge::Left, Edge::Top].into_iter().collect();
        assert!(s.contains(Edge::Left));
        assert!(s.contains(Edge::Top));
        assert!(!s.contains(Edge::Right));
        assert!(!s.is_empty());
        assert!(EdgeSet::empty().is_empty());
    }

    #[test]
    fn edge_opposite() {
        assert_eq!(Edge::Left.opposite(), Edge::Right);
        assert_eq!(Edge::Bottom.opposite(), Edge::Top);
    }
}
