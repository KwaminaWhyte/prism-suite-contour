//! Pure stroke geometry: align-stroke centerline offset and baked arrowhead
//! markers.
//!
//! These helpers are renderer-agnostic — they take and return plain
//! `(f32, f32)` polylines / polygons in document space, so the egui canvas
//! painter, the SVG exporter, and the tiny-skia PNG rasterizer all consume the
//! same geometry and draw an identical result.
//!
//! ## Align stroke
//!
//! A centered stroke straddles the path. To emulate *inside* / *outside* align
//! without per-renderer stroke-position support, we shift the **centerline** of
//! a closed path by ±`w/2` along its outward normal, then stroke that offset
//! contour centered as usual: the band then lands fully inside / outside. This
//! is the standard editor emulation and keeps every renderer's existing
//! cap/join/dash machinery untouched (the geometry changes, not the stroker).
//!
//! ## Arrowheads
//!
//! Markers are *baked*: [`arrowhead`] returns a small filled / stroked outline
//! at an endpoint, oriented along the path tangent and sized to the stroke
//! width × a scale. Baked geometry is the most portable form (no SVG `<marker>`
//! defs, no renderer-specific marker support) and renders pixel-identically
//! across all three surfaces.

use crate::document::{self, Arrowhead, StrokeAlign, StrokeStyle};

/// A point in document space.
type Pt = (f32, f32);

/// Twice the signed area of a closed polygon (shoelace). Positive when the
/// polygon is wound clockwise in a y-down space (Contour's screen convention),
/// negative when counter-clockwise. Used to make align-stroke `Inside` /
/// `Outside` independent of authoring winding.
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

/// The flattened, align-shifted **stroke geometry** for a path: its outline
/// flattened to a polyline, then (for `Inside` / `Outside`) shifted along the
/// outward normal by `w/2` so the centered stroke band lands on the requested
/// side. Returns `(points, closed_for_stroking)`.
///
/// For an open path, inside/outside still shift the centerline by `±w/2` (the
/// band moves off to one side), matching Illustrator's behaviour on open paths.
/// `Center` returns the plain flattened outline unchanged.
pub fn aligned_geometry(
    points: &[Pt],
    handles: &[Pt],
    closed: bool,
    width: f32,
    align: StrokeAlign,
) -> Vec<Pt> {
    let flat = document::flatten(points, handles, closed);
    if align == StrokeAlign::Center || width <= 0.0 || flat.len() < 2 {
        return flat;
    }
    // Outward is the polygon's exterior. `offset_contour` treats +dist as the
    // right-hand (CW, y-down) normal, which is outward for a clockwise polygon;
    // flip for a counter-clockwise one so "Outside" is always exterior.
    let mut sign = align.offset_sign();
    if closed && signed_area2(&flat) < 0.0 {
        sign = -sign;
    }
    offset_contour(&flat, sign * width * 0.5, closed)
}

/// Offset a contour (`pts`) by signed distance `dist` along its **outward**
/// normal, returning the offset polyline. Positive `dist` moves each vertex to
/// the right of the direction of travel (the polygon's outward side when the
/// contour is wound clockwise in screen space). The offset uses the
/// angle-bisector at each vertex (a miter join), clamped so a near-reversal
/// spike can't blow up.
///
/// `closed` wraps the first/last vertices so a polygon offsets without end
/// artifacts; an open polyline keeps its endpoints offset by their single
/// adjacent segment normal.
pub fn offset_contour(pts: &[Pt], dist: f32, closed: bool) -> Vec<Pt> {
    let n = pts.len();
    if n < 2 || dist == 0.0 {
        return pts.to_vec();
    }
    // Per-segment unit right-normals (rotate the segment direction by -90°:
    // (dx, dy) -> (dy, -dx) is the right-hand normal in a y-down space).
    let seg_count = if closed { n } else { n - 1 };
    let mut normals: Vec<Pt> = Vec::with_capacity(seg_count);
    for i in 0..seg_count {
        let a = pts[i];
        let b = pts[(i + 1) % n];
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let len = (dx * dx + dy * dy).sqrt();
        if len <= 1e-9 {
            normals.push((0.0, 0.0));
        } else {
            normals.push((dy / len, -dx / len));
        }
    }

    let mut out: Vec<Pt> = Vec::with_capacity(n);
    for i in 0..n {
        // The two segment normals meeting at vertex i (prev seg, next seg).
        let (prev_n, next_n) = if closed {
            let prev = (i + seg_count - 1) % seg_count;
            (normals[prev], normals[i % seg_count])
        } else if i == 0 {
            (normals[0], normals[0])
        } else if i == n - 1 {
            (normals[seg_count - 1], normals[seg_count - 1])
        } else {
            (normals[i - 1], normals[i])
        };
        // Miter direction = normalized sum of the adjacent normals.
        let mut mx = prev_n.0 + next_n.0;
        let mut my = prev_n.1 + next_n.1;
        let mlen = (mx * mx + my * my).sqrt();
        // Miter length factor = 1 / cos(theta/2); clamp so sharp corners don't
        // shoot to infinity (cap at 4× the offset, the SVG default miter limit).
        let scale = if mlen <= 1e-6 {
            // Near-reversal: fall back to the next segment normal.
            mx = next_n.0;
            my = next_n.1;
            dist
        } else {
            mx /= mlen;
            my /= mlen;
            // cos(theta/2) = (m · n) where m is the unit bisector and n a unit
            // segment normal. Miter factor is 1/cos.
            let cos_half = (mx * next_n.0 + my * next_n.1).abs().max(0.25);
            dist / cos_half
        };
        out.push((pts[i].0 + mx * scale, pts[i].1 + my * scale));
    }
    out
}

/// One arrowhead's baked geometry: an outline `polygon` (filled when
/// [`fill`](Self::fill) is true) and/or a `strokes` list of open polylines
/// (for the open chevron). Document space.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ArrowGeom {
    /// Filled outline (triangle / circle). Empty for the open chevron.
    pub polygon: Vec<Pt>,
    /// Open polylines to stroke (the chevron's two arms). Empty for filled
    /// markers.
    pub strokes: Vec<Vec<Pt>>,
    /// Whether `polygon` should be filled with the stroke colour.
    pub fill: bool,
    /// How far back along the tangent (toward the path) the path centerline
    /// should be trimmed so a filled marker's base meets the line cleanly,
    /// in document units. Zero for the circle / open chevron (they sit on the
    /// endpoint).
    pub trim: f32,
}

/// Build the baked geometry for arrowhead `kind` at endpoint `tip`, where the
/// path arrives travelling in unit direction `dir` (pointing *outward*, i.e. the
/// way the arrow points). `width` is the stroke width and `scale` the user
/// arrowhead-scale multiplier. Returns `None` for [`Arrowhead::None`] or a
/// degenerate direction.
pub fn arrowhead(kind: Arrowhead, tip: Pt, dir: Pt, width: f32, scale: f32) -> Option<ArrowGeom> {
    if kind == Arrowhead::None {
        return None;
    }
    let dlen = (dir.0 * dir.0 + dir.1 * dir.1).sqrt();
    if dlen <= 1e-9 || width <= 0.0 {
        return None;
    }
    // Unit forward (toward the tip) and unit left (perpendicular).
    let f = (dir.0 / dlen, dir.1 / dlen);
    let left = (-f.1, f.0);
    // Marker size scales with the stroke width (Illustrator-style), so a thick
    // line gets a proportionally bigger head.
    let s = width.max(0.5) * scale;
    match kind {
        Arrowhead::None => None,
        Arrowhead::Triangle => {
            // Isoceles triangle: tip at `tip`, base `len` behind it, half-width
            // `half` to each side. The base meets the line, so trim the line back
            // by `len` so it doesn't poke through the head.
            let len = s * 3.0;
            let half = s * 1.6;
            let base = (tip.0 - f.0 * len, tip.1 - f.1 * len);
            let l = (base.0 + left.0 * half, base.1 + left.1 * half);
            let r = (base.0 - left.0 * half, base.1 - left.1 * half);
            Some(ArrowGeom {
                polygon: vec![tip, l, r],
                strokes: Vec::new(),
                fill: true,
                trim: len * 0.85,
            })
        }
        Arrowhead::Open => {
            // A "V" chevron: two arms from the tip back to the two base corners.
            let len = s * 3.0;
            let half = s * 1.8;
            let base = (tip.0 - f.0 * len, tip.1 - f.1 * len);
            let l = (base.0 + left.0 * half, base.1 + left.1 * half);
            let r = (base.0 - left.0 * half, base.1 - left.1 * half);
            Some(ArrowGeom {
                polygon: Vec::new(),
                strokes: vec![vec![l, tip], vec![r, tip]],
                fill: false,
                trim: 0.0,
            })
        }
        Arrowhead::Circle => {
            // A filled dot centered on the endpoint.
            let r = s * 1.6;
            let steps = 24;
            let poly: Vec<Pt> = (0..steps)
                .map(|k| {
                    let t = k as f32 / steps as f32 * std::f32::consts::TAU;
                    (tip.0 + r * t.cos(), tip.1 + r * t.sin())
                })
                .collect();
            Some(ArrowGeom {
                polygon: poly,
                strokes: Vec::new(),
                fill: true,
                trim: 0.0,
            })
        }
    }
}

/// The baked arrowhead decorations for an **open** stroke, plus the (possibly
/// trimmed) centerline the main stroke should follow so a filled head's base
/// meets the line cleanly. `flat` is the already-flattened, align-aware open
/// polyline; `width` is the stroke width.
///
/// Returns `(decorations, trimmed_line)`. `decorations` is empty (and
/// `trimmed_line` equals `flat`) when the style carries no arrowheads or the
/// line is degenerate. Closed paths get no arrowheads (Illustrator only marks
/// open path ends), so callers should pass `closed == false` geometry.
pub fn arrow_decorations(flat: &[Pt], style: &StrokeStyle, width: f32) -> (Vec<ArrowGeom>, Vec<Pt>) {
    if !style.has_arrows() || width <= 0.0 {
        return (Vec::new(), flat.to_vec());
    }
    let Some(((spt, sdir), (ept, edir))) = endpoint_tangents(flat) else {
        return (Vec::new(), flat.to_vec());
    };
    let scale = style.arrow_scale_clamped();
    let mut decos = Vec::new();
    let mut trim_start = 0.0;
    let mut trim_end = 0.0;
    if let Some(g) = arrowhead(style.start_arrow, spt, sdir, width, scale) {
        trim_start = g.trim;
        decos.push(g);
    }
    if let Some(g) = arrowhead(style.end_arrow, ept, edir, width, scale) {
        trim_end = g.trim;
        decos.push(g);
    }
    let trimmed = trim_polyline(flat, trim_start, trim_end);
    (decos, trimmed)
}

/// The start / end endpoints of an open polyline plus the *outward* unit tangent
/// at each (start tangent points back off the line away from the path; end
/// tangent points forward off the line). Returns `None` for fewer than two
/// points or a degenerate (zero-length) line. The returned tuples are
/// `((point, outward_dir_at_start), (point, outward_dir_at_end))`.
pub fn endpoint_tangents(pts: &[Pt]) -> Option<((Pt, Pt), (Pt, Pt))> {
    let n = pts.len();
    if n < 2 {
        return None;
    }
    // Start: direction is from the second point toward the first (outward).
    let start = pts[0];
    let mut sdir = (start.0 - pts[1].0, start.1 - pts[1].1);
    let mut si = 1;
    while (sdir.0 * sdir.0 + sdir.1 * sdir.1) <= 1e-12 && si + 1 < n {
        si += 1;
        sdir = (start.0 - pts[si].0, start.1 - pts[si].1);
    }
    // End: direction is from the second-to-last point toward the last (outward).
    let end = pts[n - 1];
    let mut edir = (end.0 - pts[n - 2].0, end.1 - pts[n - 2].1);
    let mut ei = n - 2;
    while (edir.0 * edir.0 + edir.1 * edir.1) <= 1e-12 && ei > 0 {
        ei -= 1;
        edir = (end.0 - pts[ei].0, end.1 - pts[ei].1);
    }
    if (sdir.0 * sdir.0 + sdir.1 * sdir.1) <= 1e-12
        || (edir.0 * edir.0 + edir.1 * edir.1) <= 1e-12
    {
        return None;
    }
    Some(((start, sdir), (end, edir)))
}

/// Trim the open polyline `pts` back from each end by the given document-unit
/// distances so a filled arrowhead's base meets the line cleanly (no overshoot
/// through the marker). Walks inward from each end, dropping fully-consumed
/// segments and moving the surviving endpoint along its segment. Never collapses
/// below two points.
pub fn trim_polyline(pts: &[Pt], trim_start: f32, trim_end: f32) -> Vec<Pt> {
    let mut v = pts.to_vec();
    if v.len() < 2 {
        return v;
    }
    if trim_end > 0.0 {
        trim_one_end(&mut v, trim_end, true);
    }
    if trim_start > 0.0 {
        trim_one_end(&mut v, trim_start, false);
    }
    v
}

/// Trim `amount` document units off `from_end ? the back : the front` of `v`.
fn trim_one_end(v: &mut Vec<Pt>, amount: f32, from_end: bool) {
    let mut remaining = amount;
    loop {
        if v.len() < 2 {
            return;
        }
        let (tip_i, next_i) = if from_end {
            (v.len() - 1, v.len() - 2)
        } else {
            (0, 1)
        };
        let tip = v[tip_i];
        let next = v[next_i];
        let seg = (next.0 - tip.0, next.1 - tip.1);
        let seg_len = (seg.0 * seg.0 + seg.1 * seg.1).sqrt();
        if seg_len <= 1e-9 {
            // Drop the degenerate tip and retry against the next segment.
            v.remove(tip_i);
            continue;
        }
        if remaining < seg_len {
            // Move the tip inward along this segment by `remaining`.
            let t = remaining / seg_len;
            v[tip_i] = (tip.0 + seg.0 * t, tip.1 + seg.1 * t);
            return;
        }
        // Consume the whole segment: drop the tip and keep trimming. Guard the
        // two-point floor so we never collapse the line.
        if v.len() <= 2 {
            // Pull the tip almost to the neighbour rather than vanish.
            let t = (seg_len - 1e-3).max(0.0) / seg_len;
            v[tip_i] = (tip.0 + seg.0 * t, tip.1 + seg.1 * t);
            return;
        }
        v.remove(tip_i);
        remaining -= seg_len;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn offset_zero_is_identity() {
        let p = vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)];
        assert_eq!(offset_contour(&p, 0.0, false), p);
    }

    #[test]
    fn offset_open_horizontal_line_moves_along_normal() {
        // A horizontal line travelling +x has a right-normal of (0, -1) in a
        // y-down space, so a +2 offset moves it to y = -2.
        let p = vec![(0.0, 0.0), (10.0, 0.0)];
        let off = offset_contour(&p, 2.0, false);
        assert!(approx(off[0].1, -2.0), "got {:?}", off);
        assert!(approx(off[1].1, -2.0), "got {:?}", off);
        // x is unchanged for a pure horizontal line.
        assert!(approx(off[0].0, 0.0));
        assert!(approx(off[1].0, 10.0));
    }

    #[test]
    fn offset_closed_square_outward_grows_it() {
        // A clockwise (in y-down) unit square; outward (+) offset enlarges it.
        let sq = vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        let out = offset_contour(&sq, 2.0, true);
        // Each corner moves out by ~2 on each axis (miter at a right angle =
        // sqrt(2) * dist along the diagonal, i.e. 2 on each component).
        assert!(out[0].0 < 0.0 && out[0].1 < 0.0, "tl moved out: {:?}", out[0]);
        assert!(out[2].0 > 10.0 && out[2].1 > 10.0, "br moved out: {:?}", out[2]);
        // Inward (−) offset shrinks it.
        let inn = offset_contour(&sq, -2.0, true);
        assert!(inn[0].0 > 0.0 && inn[0].1 > 0.0, "tl moved in: {:?}", inn[0]);
    }

    #[test]
    fn offset_closed_square_right_angle_miter_is_exact() {
        let sq = vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        let out = offset_contour(&sq, 2.0, true);
        // Top-left corner offsets outward to (-2, -2) for a perfect 90° miter.
        assert!(approx(out[0].0, -2.0) && approx(out[0].1, -2.0), "{:?}", out[0]);
    }

    #[test]
    fn arrowhead_none_is_none() {
        assert!(arrowhead(Arrowhead::None, (0.0, 0.0), (1.0, 0.0), 2.0, 1.0).is_none());
    }

    #[test]
    fn triangle_points_at_tip_and_has_base_behind() {
        // Pointing +x: tip stays at the endpoint, base sits behind (smaller x).
        let g = arrowhead(Arrowhead::Triangle, (100.0, 50.0), (1.0, 0.0), 2.0, 1.0).unwrap();
        assert!(g.fill);
        assert_eq!(g.polygon.len(), 3);
        assert!(approx(g.polygon[0].0, 100.0) && approx(g.polygon[0].1, 50.0));
        // Both base corners are behind the tip.
        assert!(g.polygon[1].0 < 100.0 && g.polygon[2].0 < 100.0, "{:?}", g.polygon);
        // Base corners straddle the centerline symmetrically in y.
        assert!(approx(g.polygon[1].1 + g.polygon[2].1, 100.0), "{:?}", g.polygon);
        assert!(g.trim > 0.0);
    }

    #[test]
    fn triangle_scale_grows_the_head() {
        let small = arrowhead(Arrowhead::Triangle, (0.0, 0.0), (1.0, 0.0), 2.0, 1.0).unwrap();
        let big = arrowhead(Arrowhead::Triangle, (0.0, 0.0), (1.0, 0.0), 2.0, 2.0).unwrap();
        // Base is twice as far back at 2× scale.
        assert!(approx(big.polygon[1].0, small.polygon[1].0 * 2.0), "{:?} {:?}", big.polygon, small.polygon);
    }

    #[test]
    fn open_chevron_has_two_arms_no_fill() {
        let g = arrowhead(Arrowhead::Open, (10.0, 0.0), (1.0, 0.0), 2.0, 1.0).unwrap();
        assert!(!g.fill);
        assert!(g.polygon.is_empty());
        assert_eq!(g.strokes.len(), 2);
        // Both arms end at the tip.
        for arm in &g.strokes {
            assert_eq!(*arm.last().unwrap(), (10.0, 0.0));
        }
    }

    #[test]
    fn circle_is_filled_loop_centered_on_tip() {
        let g = arrowhead(Arrowhead::Circle, (5.0, 5.0), (1.0, 0.0), 2.0, 1.0).unwrap();
        assert!(g.fill);
        assert!(g.strokes.is_empty());
        assert!(g.polygon.len() >= 8);
        // Centroid is the tip.
        let (mut sx, mut sy) = (0.0, 0.0);
        for p in &g.polygon {
            sx += p.0;
            sy += p.1;
        }
        let c = (sx / g.polygon.len() as f32, sy / g.polygon.len() as f32);
        assert!(approx(c.0, 5.0) && approx(c.1, 5.0), "centroid {:?}", c);
    }

    #[test]
    fn endpoint_tangents_outward() {
        let p = vec![(0.0, 0.0), (10.0, 0.0)];
        let ((s, sd), (e, ed)) = endpoint_tangents(&p).unwrap();
        assert_eq!(s, (0.0, 0.0));
        assert_eq!(e, (10.0, 0.0));
        // Start tangent points back off the line (−x); end forward (+x).
        assert!(sd.0 < 0.0 && approx(sd.1, 0.0), "{:?}", sd);
        assert!(ed.0 > 0.0 && approx(ed.1, 0.0), "{:?}", ed);
    }

    #[test]
    fn endpoint_tangents_skips_coincident_points() {
        // Duplicated endpoints shouldn't yield a zero tangent.
        let p = vec![(0.0, 0.0), (0.0, 0.0), (10.0, 0.0), (10.0, 0.0)];
        let ((_, sd), (_, ed)) = endpoint_tangents(&p).unwrap();
        assert!(sd.0 < 0.0, "{:?}", sd);
        assert!(ed.0 > 0.0, "{:?}", ed);
    }

    #[test]
    fn trim_shortens_end_within_a_segment() {
        let p = vec![(0.0, 0.0), (10.0, 0.0)];
        let t = trim_polyline(&p, 0.0, 3.0);
        assert_eq!(t.len(), 2);
        assert!(approx(t[1].0, 7.0), "{:?}", t);
    }

    #[test]
    fn trim_shortens_start_within_a_segment() {
        let p = vec![(0.0, 0.0), (10.0, 0.0)];
        let t = trim_polyline(&p, 3.0, 0.0);
        assert!(approx(t[0].0, 3.0), "{:?}", t);
    }

    #[test]
    fn trim_drops_consumed_segments_but_keeps_two_points() {
        let p = vec![(0.0, 0.0), (5.0, 0.0), (10.0, 0.0)];
        // Trim 7 off the end: consumes the last 5-unit segment, then 2 into the
        // first, leaving the line ending at x = 3.
        let t = trim_polyline(&p, 0.0, 7.0);
        assert!(t.len() >= 2);
        assert!(approx(t.last().unwrap().0, 3.0), "{:?}", t);
    }

    #[test]
    fn trim_never_collapses_below_two_points() {
        let p = vec![(0.0, 0.0), (10.0, 0.0)];
        // Over-trim from both ends; must still return >= 2 points.
        let t = trim_polyline(&p, 100.0, 100.0);
        assert!(t.len() >= 2, "{:?}", t);
    }

    #[test]
    fn aligned_center_is_unshifted() {
        let pts = vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        let h = vec![(0.0, 0.0); 4];
        let g = aligned_geometry(&pts, &h, true, 4.0, StrokeAlign::Center);
        assert_eq!(g, pts);
    }

    #[test]
    fn aligned_outside_grows_cw_square_regardless_of_winding() {
        // Clockwise square (y-down): Outside should enlarge it.
        let cw = vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        let h = vec![(0.0, 0.0); 4];
        let out_cw = aligned_geometry(&cw, &h, true, 4.0, StrokeAlign::Outside);
        let area_cw = signed_area2(&out_cw).abs();
        assert!(area_cw > signed_area2(&cw).abs(), "outside should grow CW");

        // Counter-clockwise square: Outside must STILL enlarge it (winding-
        // independent), confirming the sign flip.
        let ccw = vec![(0.0, 0.0), (0.0, 10.0), (10.0, 10.0), (10.0, 0.0)];
        let out_ccw = aligned_geometry(&ccw, &h, true, 4.0, StrokeAlign::Outside);
        assert!(
            signed_area2(&out_ccw).abs() > signed_area2(&ccw).abs(),
            "outside should grow CCW too"
        );
    }

    #[test]
    fn aligned_inside_shrinks_square() {
        let sq = vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        let h = vec![(0.0, 0.0); 4];
        let inn = aligned_geometry(&sq, &h, true, 4.0, StrokeAlign::Inside);
        assert!(
            signed_area2(&inn).abs() < signed_area2(&sq).abs(),
            "inside should shrink"
        );
    }

    #[test]
    fn arrow_decorations_empty_when_no_arrows() {
        let flat = vec![(0.0, 0.0), (10.0, 0.0)];
        let style = StrokeStyle::default();
        let (decos, line) = arrow_decorations(&flat, &style, 2.0);
        assert!(decos.is_empty());
        assert_eq!(line, flat);
    }

    #[test]
    fn arrow_decorations_end_triangle_trims_line() {
        let flat = vec![(0.0, 0.0), (100.0, 0.0)];
        let mut style = StrokeStyle {
            end_arrow: Arrowhead::Triangle,
            ..StrokeStyle::default()
        };
        style.arrow_scale = 1.0;
        let (decos, line) = arrow_decorations(&flat, &style, 4.0);
        assert_eq!(decos.len(), 1);
        assert!(decos[0].fill);
        // The line was trimmed back from x=100 so the head's base meets it.
        assert!(line.last().unwrap().0 < 100.0, "{:?}", line);
    }

    #[test]
    fn arrow_decorations_both_ends() {
        let flat = vec![(0.0, 0.0), (100.0, 0.0)];
        let style = StrokeStyle {
            start_arrow: Arrowhead::Circle,
            end_arrow: Arrowhead::Triangle,
            ..StrokeStyle::default()
        };
        let (decos, _) = arrow_decorations(&flat, &style, 4.0);
        assert_eq!(decos.len(), 2);
    }
}
