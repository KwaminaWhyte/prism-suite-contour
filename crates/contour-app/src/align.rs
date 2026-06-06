//! Align & distribute — pure geometry over axis-aligned bounding boxes.
//!
//! Illustrator's Align panel has two families of operation:
//!
//! - **Align** snaps every object's edge or centre to a shared coordinate. The
//!   shared coordinate comes from a *reference frame* ([`AlignTo`]): the combined
//!   bounds of the selection, or the artboard rectangle.
//! - **Distribute** spreads three-or-more objects so a chosen feature (their
//!   edges or their centres) is evenly spaced, *or* so the empty gaps between
//!   them are equal ("distribute spacing").
//!
//! All of this is closed-form arithmetic on each object's bounding box, so it
//! lives here as small pure functions returning a per-object translation delta
//! `(dx, dy)`. The app layer reads each selected shape's [`bounds`], asks for the
//! deltas, and applies them with the shape's existing `translate` — one undo
//! step. Keeping it UI-free makes the spacing/edge maths unit-testable.
//!
//! [`bounds`]: crate::document::Shape::bounds

use prism_core::geometry::Rect as CoreRect;

/// The six edge/centre alignments (three per axis). Each maps every box's
/// matching feature to a single coordinate drawn from the reference frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    /// Left edges share the reference frame's left (`x`).
    Left,
    /// Horizontal centres share the reference frame's horizontal centre.
    CenterH,
    /// Right edges share the reference frame's right (`x + w`).
    Right,
    /// Top edges share the reference frame's top (`y`).
    Top,
    /// Vertical centres share the reference frame's vertical centre.
    CenterV,
    /// Bottom edges share the reference frame's bottom (`y + h`).
    Bottom,
}

/// The distribute operations. The first four equalise the spacing of a chosen
/// object feature (edge or centre); the last two equalise the *gaps* between
/// objects, which is what "Distribute Spacing" does in Illustrator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Distribute {
    /// Equalise horizontal spacing between left edges.
    LeftEdges,
    /// Equalise horizontal spacing between horizontal centres.
    CentersH,
    /// Equalise horizontal spacing between right edges.
    RightEdges,
    /// Equalise vertical spacing between top edges.
    TopEdges,
    /// Equalise vertical spacing between vertical centres.
    CentersV,
    /// Equalise vertical spacing between bottom edges.
    BottomEdges,
    /// Equalise the horizontal gaps between adjacent objects.
    HorizontalGap,
    /// Equalise the vertical gaps between adjacent objects.
    VerticalGap,
}

/// Which rectangle the [`Align`] operations measure against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlignTo {
    /// The combined bounding box of the selected objects (Illustrator default).
    Selection,
    /// A fixed artboard rectangle.
    Artboard,
}

/// The union (combined bounding box) of a set of rectangles, or `None` when the
/// slice is empty.
pub fn union_bounds(boxes: &[CoreRect]) -> Option<CoreRect> {
    let mut it = boxes.iter();
    let first = it.next()?;
    let mut min_x = first.x;
    let mut min_y = first.y;
    let mut max_x = first.x + first.w;
    let mut max_y = first.y + first.h;
    for r in it {
        min_x = min_x.min(r.x);
        min_y = min_y.min(r.y);
        max_x = max_x.max(r.x + r.w);
        max_y = max_y.max(r.y + r.h);
    }
    Some(CoreRect::new(min_x, min_y, max_x - min_x, max_y - min_y))
}

/// Per-object translation deltas to perform [`Align`] against `frame`.
///
/// Returns one `(dx, dy)` per input box, in the same order. Only the axis the
/// operation acts on is non-zero; the other stays `0.0`. An empty input yields
/// an empty result.
pub fn align_deltas(boxes: &[CoreRect], op: Align, frame: CoreRect) -> Vec<(f32, f32)> {
    boxes
        .iter()
        .map(|b| match op {
            Align::Left => (frame.x - b.x, 0.0),
            Align::Right => ((frame.x + frame.w) - (b.x + b.w), 0.0),
            Align::CenterH => {
                let fc = frame.x + frame.w * 0.5;
                let bc = b.x + b.w * 0.5;
                (fc - bc, 0.0)
            }
            Align::Top => (0.0, frame.y - b.y),
            Align::Bottom => (0.0, (frame.y + frame.h) - (b.y + b.h)),
            Align::CenterV => {
                let fc = frame.y + frame.h * 0.5;
                let bc = b.y + b.h * 0.5;
                (0.0, fc - bc)
            }
        })
        .collect()
}

/// Per-object translation deltas to perform a [`Distribute`].
///
/// Distribute needs at least three objects to be meaningful (with two there is
/// nothing to space *between*), so fewer than three returns all-zero deltas. The
/// two outermost objects on the operating axis stay put; the interior objects
/// move to even out the chosen feature or gap.
///
/// Returns one `(dx, dy)` per input box, in input order.
pub fn distribute_deltas(boxes: &[CoreRect], op: Distribute) -> Vec<(f32, f32)> {
    let n = boxes.len();
    let mut deltas = vec![(0.0f32, 0.0f32); n];
    if n < 3 {
        return deltas;
    }

    let horizontal = matches!(
        op,
        Distribute::LeftEdges
            | Distribute::CentersH
            | Distribute::RightEdges
            | Distribute::HorizontalGap
    );

    // Sort object indices by their position on the operating axis so we space
    // them in visual order regardless of selection/draw order.
    let key = |r: &CoreRect| -> f32 {
        if horizontal {
            r.x + r.w * 0.5
        } else {
            r.y + r.h * 0.5
        }
    };
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| key(&boxes[a]).total_cmp(&key(&boxes[b])));

    match op {
        Distribute::HorizontalGap | Distribute::VerticalGap => {
            // Equal empty gap between adjacent boxes. Total free space = span of
            // the outer extents minus the sum of object sizes along the axis,
            // shared equally across the (n-1) gaps.
            let size = |r: &CoreRect| if horizontal { r.w } else { r.h };
            let lo = |r: &CoreRect| if horizontal { r.x } else { r.y };
            let first = &boxes[order[0]];
            let last = &boxes[order[n - 1]];
            let span = (lo(last) + size(last)) - lo(first);
            let total_size: f32 = order.iter().map(|&i| size(&boxes[i])).sum();
            let gap = (span - total_size) / (n as f32 - 1.0);

            // Walk left/top to right/bottom, packing each interior box one `gap`
            // past the previous box's far edge. Ends are fixed.
            let mut cursor = lo(first) + size(first);
            for k in 1..n - 1 {
                let idx = order[k];
                let b = &boxes[idx];
                let target = cursor + gap;
                let d = target - lo(b);
                deltas[idx] = if horizontal { (d, 0.0) } else { (0.0, d) };
                cursor = target + size(b);
            }
        }
        _ => {
            // Equalise a single feature coordinate (edge or centre). The feature
            // of the first and last objects fixes the range; interior features
            // land on an evenly-spaced grid between them.
            let feature = |r: &CoreRect| -> f32 {
                match op {
                    Distribute::LeftEdges => r.x,
                    Distribute::RightEdges => r.x + r.w,
                    Distribute::CentersH => r.x + r.w * 0.5,
                    Distribute::TopEdges => r.y,
                    Distribute::BottomEdges => r.y + r.h,
                    Distribute::CentersV => r.y + r.h * 0.5,
                    // Gap variants handled above.
                    _ => unreachable!(),
                }
            };
            let start = feature(&boxes[order[0]]);
            let end = feature(&boxes[order[n - 1]]);
            let step = (end - start) / (n as f32 - 1.0);
            for (k, &idx) in order.iter().enumerate() {
                let target = start + step * k as f32;
                let d = target - feature(&boxes[idx]);
                deltas[idx] = if horizontal { (d, 0.0) } else { (0.0, d) };
            }
        }
    }

    deltas
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: f32, y: f32, w: f32, h: f32) -> CoreRect {
        CoreRect::new(x, y, w, h)
    }

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn union_of_two_boxes() {
        let u = union_bounds(&[r(0.0, 0.0, 10.0, 10.0), r(20.0, 5.0, 10.0, 30.0)]).unwrap();
        assert!(approx(u.x, 0.0) && approx(u.y, 0.0));
        assert!(approx(u.w, 30.0) && approx(u.h, 35.0));
    }

    #[test]
    fn union_empty_is_none() {
        assert!(union_bounds(&[]).is_none());
    }

    #[test]
    fn align_left_moves_all_to_frame_left() {
        let boxes = [r(10.0, 0.0, 4.0, 4.0), r(30.0, 0.0, 8.0, 4.0)];
        let frame = union_bounds(&boxes).unwrap(); // left edge = 10
        let d = align_deltas(&boxes, Align::Left, frame);
        assert!(approx(d[0].0, 0.0)); // already at left
        assert!(approx(d[1].0, -20.0)); // 30 -> 10
                                        // Y untouched.
        assert!(approx(d[0].1, 0.0) && approx(d[1].1, 0.0));
        // Applying the deltas lands both left edges on 10.
        assert!(approx(boxes[1].x + d[1].0, 10.0));
    }

    #[test]
    fn align_right_uses_far_edge() {
        let boxes = [r(0.0, 0.0, 4.0, 4.0), r(0.0, 0.0, 10.0, 4.0)];
        let frame = union_bounds(&boxes).unwrap(); // right edge = 10
        let d = align_deltas(&boxes, Align::Right, frame);
        // Box 0 right edge 4 -> 10, so dx = 6.
        assert!(approx(d[0].0, 6.0));
        assert!(approx(d[1].0, 0.0));
    }

    #[test]
    fn align_center_h_centers_on_frame() {
        let boxes = [r(0.0, 0.0, 10.0, 2.0), r(0.0, 0.0, 4.0, 2.0)];
        let frame = union_bounds(&boxes).unwrap(); // width 10, centre x = 5
        let d = align_deltas(&boxes, Align::CenterH, frame);
        // Box 1 centre is at 2 -> needs +3 to reach 5.
        assert!(approx(d[1].0, 3.0));
        assert!(approx(boxes[1].x + boxes[1].w * 0.5 + d[1].0, 5.0));
    }

    #[test]
    fn align_top_and_bottom() {
        let boxes = [r(0.0, 5.0, 4.0, 4.0), r(0.0, 20.0, 4.0, 10.0)];
        let frame = union_bounds(&boxes).unwrap(); // top 5, bottom 30
        let top = align_deltas(&boxes, Align::Top, frame);
        assert!(approx(top[1].1, -15.0)); // 20 -> 5
        let bottom = align_deltas(&boxes, Align::Bottom, frame);
        // Box 0 bottom 9 -> 30, dy = 21.
        assert!(approx(bottom[0].1, 21.0));
    }

    #[test]
    fn align_to_artboard_frame() {
        // A single box aligned to a fixed artboard rect, not its own bounds.
        let boxes = [r(100.0, 100.0, 20.0, 20.0)];
        let artboard = r(0.0, 0.0, 1000.0, 700.0);
        let d = align_deltas(&boxes, Align::CenterH, artboard);
        // Artboard centre x = 500; box centre = 110; dx = 390.
        assert!(approx(d[0].0, 390.0));
        let d2 = align_deltas(&boxes, Align::Bottom, artboard);
        // Artboard bottom = 700; box bottom = 120; dy = 580.
        assert!(approx(d2[0].1, 580.0));
    }

    #[test]
    fn distribute_needs_three() {
        let boxes = [r(0.0, 0.0, 4.0, 4.0), r(100.0, 0.0, 4.0, 4.0)];
        let d = distribute_deltas(&boxes, Distribute::CentersH);
        assert_eq!(d, vec![(0.0, 0.0), (0.0, 0.0)]);
    }

    #[test]
    fn distribute_centers_h_evens_spacing() {
        // Three boxes, centres at 2, 30, 102. Even centre spacing => middle
        // centre should land midway between 2 and 102, i.e. 52.
        let boxes = [
            r(0.0, 0.0, 4.0, 4.0),   // centre 2
            r(28.0, 0.0, 4.0, 4.0),  // centre 30
            r(100.0, 0.0, 4.0, 4.0), // centre 102
        ];
        let d = distribute_deltas(&boxes, Distribute::CentersH);
        // Ends fixed.
        assert!(approx(d[0].0, 0.0));
        assert!(approx(d[2].0, 0.0));
        // Middle centre 30 -> 52, dx = 22.
        assert!(approx(d[1].0, 22.0));
        // Y untouched.
        assert!(d.iter().all(|&(_, dy)| approx(dy, 0.0)));
    }

    #[test]
    fn distribute_horizontal_gap_equalizes_gaps() {
        // Widths 10, 20, 10 packed in [0, 100]. Free space = 100 - 40 = 60 over
        // 2 gaps => 30 each. Layout: [0,10] gap30 [40,60] gap30 [90,100].
        let boxes = [
            r(0.0, 0.0, 10.0, 4.0),
            r(15.0, 0.0, 20.0, 4.0), // misplaced; should move to x=40
            r(90.0, 0.0, 10.0, 4.0),
        ];
        let d = distribute_deltas(&boxes, Distribute::HorizontalGap);
        assert!(approx(d[0].0, 0.0)); // first fixed
        assert!(approx(d[2].0, 0.0)); // last fixed
                                      // Middle box left edge 15 -> 40, dx = 25.
        assert!(approx(d[1].0, 25.0));
        // Verify the two resulting gaps are equal.
        let g1 = (boxes[1].x + d[1].0) - (boxes[0].x + boxes[0].w);
        let g2 = (boxes[2].x + d[2].0) - (boxes[1].x + d[1].0 + boxes[1].w);
        assert!(approx(g1, g2));
        assert!(approx(g1, 30.0));
    }

    #[test]
    fn distribute_respects_visual_order_not_input_order() {
        // Same three boxes as the gap test but listed out of order; the result
        // must still produce equal gaps left-to-right.
        let boxes = [
            r(90.0, 0.0, 10.0, 4.0), // rightmost, listed first
            r(0.0, 0.0, 10.0, 4.0),  // leftmost
            r(15.0, 0.0, 20.0, 4.0), // middle
        ];
        let d = distribute_deltas(&boxes, Distribute::HorizontalGap);
        // Leftmost (idx 1) and rightmost (idx 0) are the fixed ends.
        assert!(approx(d[0].0, 0.0));
        assert!(approx(d[1].0, 0.0));
        // Middle (idx 2) moves to x = 40.
        assert!(approx(boxes[2].x + d[2].0, 40.0));
    }

    #[test]
    fn distribute_vertical_gap_uses_y_axis() {
        let boxes = [
            r(0.0, 0.0, 4.0, 10.0),
            r(0.0, 15.0, 4.0, 20.0),
            r(0.0, 90.0, 4.0, 10.0),
        ];
        let d = distribute_deltas(&boxes, Distribute::VerticalGap);
        // Only the y component should move.
        assert!(d.iter().all(|&(dx, _)| approx(dx, 0.0)));
        assert!(approx(boxes[1].y + d[1].0, 0.0) || approx(boxes[1].y + d[1].1, 40.0));
        assert!(approx(boxes[1].y + d[1].1, 40.0));
    }

    #[test]
    fn distribute_left_edges() {
        // Left edges at 0, 5, 100 -> evenly spaced means 0, 50, 100.
        let boxes = [
            r(0.0, 0.0, 4.0, 4.0),
            r(5.0, 0.0, 8.0, 4.0),
            r(100.0, 0.0, 4.0, 4.0),
        ];
        let d = distribute_deltas(&boxes, Distribute::LeftEdges);
        assert!(approx(boxes[1].x + d[1].0, 50.0));
        assert!(approx(d[0].0, 0.0) && approx(d[2].0, 0.0));
    }
}
