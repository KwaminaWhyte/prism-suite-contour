//! Pure path-editing geometry: **Simplify** (anchor reduction), **Offset
//! Path** (signed inset / outset), and **Outline Stroke** (a path's stroke
//! converted to a filled outline band).
//!
//! Like [`crate::stroke`], these are renderer- and UI-agnostic: they take and
//! return plain `(f32, f32)` polylines in document space, so they unit-test
//! headlessly. The app wires them onto the selected path as one undo step (see
//! `app::edit`), flattening any bezier handles first and producing a plain
//! corner path (the result is no longer parametric, matching Illustrator's
//! demote-to-path behaviour of these commands).

/// A point in document space.
type Pt = (f32, f32);

/// **Simplify** a flattened contour with the Douglas–Peucker line-reduction
/// algorithm: drop interior vertices that lie within `tolerance` document units
/// of the chord they would be skipped across, preserving the overall shape while
/// cutting redundant anchors. Larger `tolerance` removes more points.
///
/// Endpoints are always preserved. For a `closed` contour the first vertex is
/// pinned and the ring is simplified as an open run from it back to itself, so a
/// closed shape stays closed (the caller keeps the `closed` flag) and the result
/// never collapses below 3 points; an open path never collapses below 2.
///
/// A non-positive `tolerance`, or a contour already at the floor, returns the
/// input unchanged (idempotent), so re-running Simplify on a minimal path is a
/// no-op.
pub fn simplify(pts: &[Pt], closed: bool, tolerance: f32) -> Vec<Pt> {
    let n = pts.len();
    let floor = if closed { 3 } else { 2 };
    if n <= floor || tolerance <= 0.0 {
        return pts.to_vec();
    }
    if closed {
        // Treat the ring as an open run start -> ... -> start. Douglas–Peucker
        // on that run pins the first vertex; we then drop the duplicated closing
        // copy so the caller's `closed` flag re-wraps it.
        let mut run: Vec<Pt> = pts.to_vec();
        run.push(pts[0]);
        let mut kept = douglas_peucker(&run, tolerance);
        // Remove the trailing duplicate of the first point.
        if kept.len() >= 2 && kept.first() == kept.last() {
            kept.pop();
        }
        if kept.len() < floor {
            return pts.to_vec();
        }
        kept
    } else {
        let kept = douglas_peucker(pts, tolerance);
        if kept.len() < floor {
            return pts.to_vec();
        }
        kept
    }
}

/// Douglas–Peucker on an **open** polyline: keep the two endpoints and any
/// vertex whose perpendicular distance to the current chord exceeds `tol`,
/// recursing on each side of the farthest such vertex. Returns the kept points
/// in order. Iterative (explicit stack) so deep paths can't blow the call stack.
fn douglas_peucker(pts: &[Pt], tol: f32) -> Vec<Pt> {
    let n = pts.len();
    if n < 3 {
        return pts.to_vec();
    }
    let mut keep = vec![false; n];
    keep[0] = true;
    keep[n - 1] = true;
    // Stack of (start, end) inclusive index ranges to subdivide.
    let mut stack: Vec<(usize, usize)> = vec![(0, n - 1)];
    while let Some((start, end)) = stack.pop() {
        if end <= start + 1 {
            continue;
        }
        let a = pts[start];
        let b = pts[end];
        let mut max_d = -1.0f32;
        let mut max_i = start;
        for (i, &p) in pts.iter().enumerate().take(end).skip(start + 1) {
            let d = perp_distance(p, a, b);
            if d > max_d {
                max_d = d;
                max_i = i;
            }
        }
        if max_d > tol {
            keep[max_i] = true;
            stack.push((start, max_i));
            stack.push((max_i, end));
        }
    }
    pts.iter()
        .zip(keep)
        .filter_map(|(&p, k)| k.then_some(p))
        .collect()
}

/// Perpendicular distance from point `p` to the line through `a`–`b` (or to the
/// point `a` when `a == b`).
fn perp_distance(p: Pt, a: Pt, b: Pt) -> f32 {
    let dx = b.0 - a.0;
    let dy = b.1 - a.1;
    let len2 = dx * dx + dy * dy;
    if len2 <= 1e-12 {
        return (p.0 - a.0).hypot(p.1 - a.1);
    }
    // |cross((p - a), (b - a))| / |b - a|.
    let cross = (p.0 - a.0) * dy - (p.1 - a.1) * dx;
    cross.abs() / len2.sqrt()
}

/// **Offset Path**: produce a new contour offset by the signed `distance` from a
/// flattened source contour, using a **miter-join** polygon offset (the same
/// angle-bisector offset the stroke-align code uses, with a miter clamp so sharp
/// corners can't spike).
///
/// The sign follows the document's y-down screen convention: for a *closed*
/// contour, positive `distance` grows it outward (outset) and negative shrinks
/// it inward (inset), independent of the source winding. For an *open* contour
/// the whole polyline shifts to one side by `distance` along its normal.
///
/// `distance == 0`, or a degenerate contour, returns the input unchanged
/// (identity).
pub fn offset_path(pts: &[Pt], closed: bool, distance: f32) -> Vec<Pt> {
    if distance == 0.0 || pts.len() < 2 {
        return pts.to_vec();
    }
    // `stroke::offset_contour` treats +dist as the right-hand (CW, y-down)
    // normal, which is *outward* only for a clockwise ring. Normalise so +dist
    // always grows a closed contour regardless of authoring winding.
    let mut sign = 1.0;
    if closed && signed_area2(pts) < 0.0 {
        sign = -1.0;
    }
    crate::stroke::offset_contour(pts, sign * distance, closed)
}

/// Twice the signed area (shoelace) of a closed polygon. Positive when wound
/// clockwise in y-down space, negative counter-clockwise — used to make
/// [`offset_path`]'s grow/shrink sense winding-independent.
fn signed_area2(pts: &[Pt]) -> f32 {
    let n = pts.len();
    if n < 3 {
        return 0.0;
    }
    let mut acc = 0.0;
    for i in 0..n {
        let a = pts[i];
        let b = pts[(i + 1) % n];
        acc += a.0 * b.1 - b.0 * a.1;
    }
    acc
}

/// **Outline Stroke**: convert a path's *stroke* into a filled outline — the
/// region the centred stroke of half-width `half_w` covers — returning one or
/// more closed contours (Illustrator's `Object ▸ Path ▸ Outline Stroke`). The
/// caller paints the result with the former stroke colour as its **fill** and no
/// stroke.
///
/// - An **open** path becomes a single closed **band** that runs down one
///   `+half_w` offset and back along the `-half_w` offset (butt caps at the
///   ends), so the band's area ≈ path length × stroke width.
/// - A **closed** path becomes an **annulus**: two contours, an outer `+half_w`
///   ring and an inner `-half_w` ring, to be filled even-odd so the interior
///   hole is carved (the stroke ring is the filled region between them).
///
/// A non-positive `half_w`, or a degenerate contour (< 2 points), yields no
/// contours (the caller treats this as a no-op — nothing to outline).
pub fn outline_stroke(pts: &[Pt], closed: bool, half_w: f32) -> Vec<Vec<Pt>> {
    if half_w <= 0.0 || pts.len() < 2 {
        return Vec::new();
    }
    if closed {
        // Annulus: outer ring outset by +half_w, inner ring inset by half_w.
        // `offset_path` makes the grow/shrink sense winding-independent, so the
        // outer always encloses the inner regardless of authoring direction.
        let outer = offset_path(pts, true, half_w);
        let inner = offset_path(pts, true, -half_w);
        if outer.len() < 3 || inner.len() < 3 {
            return Vec::new();
        }
        vec![outer, inner]
    } else {
        // Open band: walk one side forward, the other side back, into one ring.
        // `offset_contour` shifts the whole polyline ±half_w along its normal;
        // the two sides sit on opposite sides of the centreline (butt caps).
        let left = crate::stroke::offset_contour(pts, half_w, false);
        let right = crate::stroke::offset_contour(pts, -half_w, false);
        if left.len() < 2 || right.len() < 2 {
            return Vec::new();
        }
        let mut band = left;
        band.extend(right.into_iter().rev());
        vec![band]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bbox(pts: &[Pt]) -> (f32, f32, f32, f32) {
        let mut it = pts.iter();
        let &(x0, y0) = it.next().unwrap();
        let (mut min_x, mut min_y, mut max_x, mut max_y) = (x0, y0, x0, y0);
        for &(x, y) in it {
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
        }
        (min_x, min_y, max_x, max_y)
    }

    fn area(pts: &[Pt]) -> f32 {
        signed_area2(pts).abs() * 0.5
    }

    fn approx(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() <= eps
    }

    // --- Simplify ------------------------------------------------------------

    #[test]
    fn simplify_reduces_collinear_redundant_anchors() {
        // A straight diagonal sampled at many points collapses to its endpoints.
        let pts: Vec<Pt> = (0..=10).map(|i| (i as f32, i as f32)).collect();
        let out = simplify(&pts, false, 0.1);
        assert!(out.len() < pts.len(), "should drop interior points: {out:?}");
        // Endpoints preserved.
        assert_eq!(*out.first().unwrap(), (0.0, 0.0));
        assert_eq!(*out.last().unwrap(), (10.0, 10.0));
        // A perfectly straight run reduces to exactly its two endpoints.
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn simplify_preserves_rough_shape_endpoints() {
        // An L-shape with redundant midpoints on each leg: the corner survives.
        let pts = vec![
            (0.0, 0.0),
            (5.0, 0.0),
            (10.0, 0.0), // redundant (collinear on the bottom leg)
            (10.0, 5.0),
            (10.0, 10.0), // redundant (collinear on the right leg)
        ];
        let out = simplify(&pts, false, 0.1);
        // First/last kept; the genuine corner (10,0) kept; redundant midpoints
        // dropped.
        assert_eq!(*out.first().unwrap(), (0.0, 0.0));
        assert_eq!(*out.last().unwrap(), (10.0, 10.0));
        assert!(out.contains(&(10.0, 0.0)), "corner kept: {out:?}");
        assert_eq!(out.len(), 3, "two legs => 3 anchors: {out:?}");
    }

    #[test]
    fn simplify_already_minimal_is_idempotent() {
        let tri = vec![(0.0, 0.0), (10.0, 0.0), (5.0, 8.0)];
        let out = simplify(&tri, true, 0.5);
        assert_eq!(out, tri, "minimal triangle unchanged");
    }

    #[test]
    fn simplify_closed_square_with_edge_midpoints_drops_them() {
        // A square whose edges carry redundant midpoints; simplify recovers the
        // 4 corners and stays a closed ring (no duplicated closing vertex).
        let sq = vec![
            (0.0, 0.0),
            (5.0, 0.0),
            (10.0, 0.0),
            (10.0, 5.0),
            (10.0, 10.0),
            (5.0, 10.0),
            (0.0, 10.0),
            (0.0, 5.0),
        ];
        let out = simplify(&sq, true, 0.1);
        assert_eq!(out.len(), 4, "four corners: {out:?}");
        // First vertex pinned; no closing duplicate.
        assert_eq!(out[0], (0.0, 0.0));
        assert_ne!(*out.last().unwrap(), out[0]);
    }

    #[test]
    fn simplify_zero_tolerance_is_identity() {
        let pts: Vec<Pt> = (0..=5).map(|i| (i as f32, 0.0)).collect();
        assert_eq!(simplify(&pts, false, 0.0), pts);
    }

    #[test]
    fn simplify_is_deterministic() {
        let pts: Vec<Pt> = (0..=20)
            .map(|i| (i as f32, (i as f32 * 0.7).sin()))
            .collect();
        let a = simplify(&pts, false, 0.3);
        let b = simplify(&pts, false, 0.3);
        assert_eq!(a, b);
    }

    #[test]
    fn simplify_never_collapses_closed_below_three() {
        let tri = vec![(0.0, 0.0), (10.0, 0.0), (5.0, 8.0)];
        // A giant tolerance would drop everything, but a closed ring floors at 3.
        let out = simplify(&tri, true, 1000.0);
        assert!(out.len() >= 3, "{out:?}");
    }

    // --- Offset Path ---------------------------------------------------------

    #[test]
    fn offset_zero_is_identity() {
        let sq = vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        assert_eq!(offset_path(&sq, true, 0.0), sq);
    }

    #[test]
    fn offset_positive_grows_bbox_negative_shrinks() {
        let sq = vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        let base = bbox(&sq);
        let base_area = area(&sq);

        let out = offset_path(&sq, true, 2.0);
        let ob = bbox(&out);
        assert!(ob.0 < base.0 && ob.1 < base.1, "min grows out: {ob:?}");
        assert!(ob.2 > base.2 && ob.3 > base.3, "max grows out: {ob:?}");
        assert!(area(&out) > base_area, "outset grows area");

        let inn = offset_path(&sq, true, -2.0);
        assert!(area(&inn) < base_area, "inset shrinks area");
    }

    #[test]
    fn offset_grows_outward_regardless_of_winding() {
        // Clockwise (y-down) and counter-clockwise squares both grow on +dist.
        let cw = vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        let ccw = vec![(0.0, 0.0), (0.0, 10.0), (10.0, 10.0), (10.0, 0.0)];
        assert!(area(&offset_path(&cw, true, 2.0)) > area(&cw));
        assert!(area(&offset_path(&ccw, true, 2.0)) > area(&ccw));
    }

    #[test]
    fn offset_right_angle_is_exact_miter() {
        let cw = vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        let out = offset_path(&cw, true, 2.0);
        // Top-left corner moves out to (-2, -2) for a perfect 90° miter.
        assert!(approx(out[0].0, -2.0, 1e-3) && approx(out[0].1, -2.0, 1e-3), "{:?}", out[0]);
    }

    #[test]
    fn offset_open_line_shifts_along_normal() {
        let line = vec![(0.0, 0.0), (10.0, 0.0)];
        let out = offset_path(&line, false, 2.0);
        // x preserved, y shifted by the normal (sign per the right-hand rule).
        assert!(approx(out[0].0, 0.0, 1e-3) && approx(out[1].0, 10.0, 1e-3));
        assert!(approx(out[0].1.abs(), 2.0, 1e-3), "shifted by 2: {out:?}");
    }

    #[test]
    fn offset_is_deterministic() {
        let sq = vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        assert_eq!(offset_path(&sq, true, 3.0), offset_path(&sq, true, 3.0));
    }

    // --- Outline Stroke ------------------------------------------------------

    #[test]
    fn outline_open_segment_is_band_of_length_by_width() {
        // A horizontal segment of length 10, stroke width 4 (half = 2), outlines
        // to one closed band whose bbox ≈ length × width.
        let line = vec![(0.0, 0.0), (10.0, 0.0)];
        let out = outline_stroke(&line, false, 2.0);
        assert_eq!(out.len(), 1, "open path => single band: {out:?}");
        let band = &out[0];
        assert!(band.len() >= 4, "band has >= 4 corners: {band:?}");
        let (min_x, min_y, max_x, max_y) = bbox(band);
        assert!(approx(max_x - min_x, 10.0, 1e-3), "band length ≈ 10: {band:?}");
        assert!(approx(max_y - min_y, 4.0, 1e-3), "band width ≈ 4: {band:?}");
        // The band encloses real area (length × width).
        assert!(approx(area(band), 40.0, 1e-2), "band area ≈ 40: {}", area(band));
    }

    #[test]
    fn outline_closed_path_is_annulus_with_positive_area() {
        // A closed square outlined yields two rings (outer + inner); the region
        // between them (outer area − inner area) is positive.
        let sq = vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        let out = outline_stroke(&sq, true, 2.0);
        assert_eq!(out.len(), 2, "closed path => annulus (outer + inner)");
        let outer_a = area(&out[0]);
        let inner_a = area(&out[1]);
        assert!(outer_a > inner_a, "outer encloses inner: {outer_a} vs {inner_a}");
        assert!(outer_a - inner_a > 0.0, "stroke band has positive area");
    }

    #[test]
    fn outline_zero_width_is_noop() {
        let line = vec![(0.0, 0.0), (10.0, 0.0)];
        assert!(outline_stroke(&line, false, 0.0).is_empty());
        let sq = vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        assert!(outline_stroke(&sq, true, -1.0).is_empty(), "negative => no-op");
    }

    #[test]
    fn outline_degenerate_is_noop() {
        assert!(outline_stroke(&[(0.0, 0.0)], false, 2.0).is_empty());
        assert!(outline_stroke(&[], true, 2.0).is_empty());
    }

    #[test]
    fn outline_is_deterministic() {
        let line = vec![(0.0, 0.0), (10.0, 0.0), (10.0, 5.0)];
        assert_eq!(outline_stroke(&line, false, 1.5), outline_stroke(&line, false, 1.5));
        let sq = vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        assert_eq!(outline_stroke(&sq, true, 1.5), outline_stroke(&sq, true, 1.5));
    }
}
