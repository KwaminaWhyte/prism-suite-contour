//! Multiple artboards — Contour's analog of Illustrator's artboards.
//!
//! An [`Artboard`] is a named rectangle in document space. A document holds an
//! ordered list of them with one *active*; the active artboard frames align-to,
//! per-artboard export, and the "new document" canvas. Drawing is not clipped to
//! an artboard (artwork can overlap the canvas the way Illustrator allows), but
//! export crops to the chosen artboard's rectangle.
//!
//! The pure geometry here — default placement of a fresh artboard, hit-testing a
//! point against the stack, and the union of all artboard rects — lives in this
//! module so it is unit-testable without any UI. The functions never mutate the
//! caller's data.

use serde::{Deserialize, Serialize};

/// One artboard: a named rectangle `[x, y, w, h]` in document units.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Artboard {
    /// Human label shown on canvas and in the Artboards list.
    pub name: String,
    /// `[x, y, w, h]` in document space.
    pub rect: [f32; 4],
}

impl Artboard {
    /// A new artboard with the given name and rectangle. Width/height are
    /// clamped non-negative.
    pub fn new(name: impl Into<String>, rect: [f32; 4]) -> Self {
        Self {
            name: name.into(),
            rect: [rect[0], rect[1], rect[2].max(0.0), rect[3].max(0.0)],
        }
    }

    /// Whether the document-space point `(x, y)` lies within this artboard.
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.rect[0]
            && x <= self.rect[0] + self.rect[2]
            && y >= self.rect[1]
            && y <= self.rect[1] + self.rect[3]
    }
}

/// Default label for the artboard at zero-based index `i` (Illustrator numbers
/// artboards from 1).
pub fn default_name(i: usize) -> String {
    format!("Artboard {}", i + 1)
}

/// Pick the topmost artboard (last in paint order) containing the point
/// `(x, y)`, returning its index. `None` when the point is on no artboard.
pub fn artboard_at(boards: &[Artboard], x: f32, y: f32) -> Option<usize> {
    boards
        .iter()
        .enumerate()
        .rev()
        .find(|(_, a)| a.contains(x, y))
        .map(|(i, _)| i)
}

/// The placement `[x, y, w, h]` for a *new* artboard given the existing stack:
/// the same size as the supplied `template` (e.g. the active artboard), placed
/// immediately to the right of the rightmost existing artboard with a gap, so
/// new artboards tile across the canvas the way Illustrator's "New Artboard"
/// does. With no existing artboards it is placed at the origin.
pub fn next_placement(boards: &[Artboard], template: [f32; 2], gap: f32) -> [f32; 4] {
    let (w, h) = (template[0].max(1.0), template[1].max(1.0));
    match boards
        .iter()
        .map(|a| a.rect[0] + a.rect[2])
        .fold(None, max_f)
    {
        // Align the new board's top with the topmost existing board's top.
        Some(right) => {
            let top = boards
                .iter()
                .map(|a| a.rect[1])
                .fold(None, min_f)
                .unwrap_or(0.0);
            [right + gap, top, w, h]
        }
        None => [0.0, 0.0, w, h],
    }
}

/// The axis-aligned union `[x, y, w, h]` of every artboard rectangle, or `None`
/// when there are no artboards. Used to frame the whole canvas (e.g. zoom-to-fit
/// across all artboards).
pub fn union_rect(boards: &[Artboard]) -> Option<[f32; 4]> {
    let mut it = boards.iter();
    let first = it.next()?;
    let (mut min_x, mut min_y) = (first.rect[0], first.rect[1]);
    let (mut max_x, mut max_y) = (first.rect[0] + first.rect[2], first.rect[1] + first.rect[3]);
    for a in it {
        min_x = min_x.min(a.rect[0]);
        min_y = min_y.min(a.rect[1]);
        max_x = max_x.max(a.rect[0] + a.rect[2]);
        max_y = max_y.max(a.rect[1] + a.rect[3]);
    }
    Some([min_x, min_y, max_x - min_x, max_y - min_y])
}

/// Fold helper: running maximum of an `f32` stream as an `Option`.
fn max_f(acc: Option<f32>, v: f32) -> Option<f32> {
    Some(acc.map_or(v, |a| a.max(v)))
}

/// Fold helper: running minimum of an `f32` stream as an `Option`.
fn min_f(acc: Option<f32>, v: f32) -> Option<f32> {
    Some(acc.map_or(v, |a| a.min(v)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ab(x: f32, y: f32, w: f32, h: f32) -> Artboard {
        Artboard::new("a", [x, y, w, h])
    }

    #[test]
    fn new_clamps_negative_extent() {
        let a = Artboard::new("x", [10.0, 20.0, -5.0, -7.0]);
        assert_eq!(a.rect, [10.0, 20.0, 0.0, 0.0]);
    }

    #[test]
    fn contains_includes_edges() {
        let a = ab(10.0, 20.0, 100.0, 40.0);
        assert!(a.contains(10.0, 20.0)); // top-left corner included
        assert!(a.contains(110.0, 60.0)); // bottom-right corner included
        assert!(a.contains(60.0, 40.0)); // centre
        assert!(!a.contains(9.0, 40.0)); // just left of edge
        assert!(!a.contains(60.0, 61.0)); // just below edge
    }

    #[test]
    fn default_name_is_one_based() {
        assert_eq!(default_name(0), "Artboard 1");
        assert_eq!(default_name(4), "Artboard 5");
    }

    #[test]
    fn artboard_at_picks_topmost() {
        // Two overlapping boards: the later one (index 1) is on top.
        let boards = vec![ab(0.0, 0.0, 100.0, 100.0), ab(50.0, 50.0, 100.0, 100.0)];
        assert_eq!(artboard_at(&boards, 75.0, 75.0), Some(1)); // overlap → top
        assert_eq!(artboard_at(&boards, 10.0, 10.0), Some(0)); // only board 0
        assert_eq!(artboard_at(&boards, 200.0, 200.0), None); // off all boards
    }

    #[test]
    fn next_placement_origin_when_empty() {
        let p = next_placement(&[], [800.0, 600.0], 40.0);
        assert_eq!(p, [0.0, 0.0, 800.0, 600.0]);
    }

    #[test]
    fn next_placement_tiles_to_the_right() {
        let boards = vec![ab(0.0, 0.0, 1000.0, 700.0)];
        let p = next_placement(&boards, [400.0, 300.0], 40.0);
        // Placed gap past the right edge (1000), top aligned with existing top.
        assert_eq!(p, [1040.0, 0.0, 400.0, 300.0]);
    }

    #[test]
    fn next_placement_uses_rightmost_and_topmost() {
        let boards = vec![
            ab(0.0, 100.0, 200.0, 200.0),  // right edge 200
            ab(500.0, 50.0, 300.0, 100.0), // right edge 800, top 50 (highest)
        ];
        let p = next_placement(&boards, [120.0, 90.0], 10.0);
        assert_eq!(p, [810.0, 50.0, 120.0, 90.0]);
    }

    #[test]
    fn union_rect_spans_all() {
        assert_eq!(union_rect(&[]), None);
        let boards = vec![ab(0.0, 0.0, 100.0, 100.0), ab(200.0, 50.0, 100.0, 100.0)];
        assert_eq!(union_rect(&boards), Some([0.0, 0.0, 300.0, 150.0]));
    }
}
