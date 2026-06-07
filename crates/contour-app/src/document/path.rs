//! Pure path geometry and direct-select editing helpers.
//!
//! These operate on the `(points, handles, closed)` triple that backs a
//! [`Shape::Path`](super::Shape::Path), plus the low-level hit-test primitives
//! shared by [`Shape::hit`](super::Shape::hit). They keep `handles` the same
//! length as `points` and are written to be unit-testable without any UI.

use kurbo::{BezPath, PathEl, Point};
use serde::{Deserialize, Serialize};

/// Which fill rule a [`Shape::Compound`](super::Shape::Compound) uses to decide
/// what is inside when its sub-contours overlap or nest — the two compound-path
/// fill rules Illustrator exposes. Mirrors
/// [`BoolFillRule`](crate::boolean::BoolFillRule) but lives on the document model
/// (and serializes), so a compound path keeps its rule in `.contour`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum FillRule {
    /// Non-zero winding: inside when the signed crossing count is non-zero.
    /// Same-direction nested rings stay filled (solid).
    #[default]
    NonZero,
    /// Even-odd: inside when an odd number of rings enclose the point, so a ring
    /// drawn inside another carves a hole (the classic donut rule).
    EvenOdd,
}

/// One sub-contour of a compound path: a (possibly cubic-bezier) ring with the
/// same `(points, handles, closed)` shape that backs a single
/// [`Shape::Path`](super::Shape::Path). A compound path is an *ordered* list of
/// these, all painted as one object with a shared fill rule, so an outer ring
/// plus inner hole rings live together rather than as separate shapes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SubPath {
    pub points: Vec<(f32, f32)>,
    #[serde(default)]
    pub handles: Vec<(f32, f32)>,
    #[serde(default = "sub_default_closed")]
    pub closed: bool,
}

/// Serde default for [`SubPath::closed`]: a compound sub-contour is closed by
/// default (an outer ring / hole), so a minimal hand-written sub-path loads closed.
fn sub_default_closed() -> bool {
    true
}

impl SubPath {
    /// A closed corner sub-contour from a ring of points (all corners).
    pub fn ring(points: Vec<(f32, f32)>) -> Self {
        let n = points.len();
        Self {
            points,
            handles: vec![(0.0, 0.0); n],
            closed: true,
        }
    }

    /// This sub-contour flattened to a document-space polyline (curves honoured).
    pub fn flatten(&self) -> Vec<(f32, f32)> {
        flatten(&self.points, &self.handles, self.closed)
    }

    /// Signed area (shoelace) of this sub-contour's flattened ring. Positive for
    /// counter-clockwise, negative for clockwise — the sign of an outer ring vs a
    /// hole under the even-odd / non-zero rules.
    pub fn signed_area(&self) -> f32 {
        let pts = self.flatten();
        let n = pts.len();
        if n < 3 {
            return 0.0;
        }
        let mut a = 0.0;
        for i in 0..n {
            let (x0, y0) = pts[i];
            let (x1, y1) = pts[(i + 1) % n];
            a += x0 * y1 - x1 * y0;
        }
        a * 0.5
    }
}

/// Whether a document-space point is inside a set of (flattened) sub-contour
/// rings under the given [`FillRule`]. Even-odd parity-counts ring crossings;
/// non-zero sums signed winding so same-direction nested rings stay solid and a
/// reverse-wound inner ring carves a hole. This is the compound-path fill test
/// shared by hit-testing and any rasterizer that needs a CPU containment check.
pub fn point_in_rings(px: f32, py: f32, rings: &[Vec<(f32, f32)>], rule: FillRule) -> bool {
    match rule {
        FillRule::EvenOdd => {
            let mut inside = false;
            for ring in rings {
                if ring.len() >= 3 && point_in_polygon(px, py, ring) {
                    inside = !inside;
                }
            }
            inside
        }
        FillRule::NonZero => winding_number(px, py, rings) != 0,
    }
}

/// Sum of the signed winding numbers of `(px, py)` against each ring (the
/// non-zero fill test's core). A crossing where the edge goes upward counts +1,
/// downward −1.
fn winding_number(px: f32, py: f32, rings: &[Vec<(f32, f32)>]) -> i32 {
    let mut wn = 0;
    for ring in rings {
        let n = ring.len();
        if n < 3 {
            continue;
        }
        for i in 0..n {
            let (x0, y0) = ring[i];
            let (x1, y1) = ring[(i + 1) % n];
            if y0 <= py {
                if y1 > py && is_left(x0, y0, x1, y1, px, py) > 0.0 {
                    wn += 1;
                }
            } else if y1 <= py && is_left(x0, y0, x1, y1, px, py) < 0.0 {
                wn -= 1;
            }
        }
    }
    wn
}

/// Positive if `(px, py)` is left of the directed edge `(x0,y0)→(x1,y1)`,
/// negative if right, zero on the line — the 2D cross product the winding-number
/// test uses to count crossings.
fn is_left(x0: f32, y0: f32, x1: f32, y1: f32, px: f32, py: f32) -> f32 {
    (x1 - x0) * (py - y0) - (px - x0) * (y1 - y0)
}

/// Whether two axis-aligned rectangles (each `[x, y, w, h]`, width/height
/// assumed non-negative) overlap. Edge-touching counts as an overlap. Used by
/// marquee (rubber-band) selection to pick every shape whose bounding box
/// intersects the dragged box.
pub fn rects_intersect(a: &[f32; 4], b: &[f32; 4]) -> bool {
    let (ax0, ay0, ax1, ay1) = (a[0], a[1], a[0] + a[2], a[1] + a[3]);
    let (bx0, by0, bx1, by1) = (b[0], b[1], b[0] + b[2], b[1] + b[3]);
    ax0 <= bx1 && bx0 <= ax1 && ay0 <= by1 && by0 <= ay1
}

/// The out-tangent handle offset for anchor `i`, or `(0, 0)` (a corner) when no
/// handle is stored.
pub fn handle_at(handles: &[(f32, f32)], i: usize) -> (f32, f32) {
    handles.get(i).copied().unwrap_or((0.0, 0.0))
}

/// The document-space positions of anchor `i`'s tangent control knobs: the
/// out-handle (`anchor + offset`) and the mirrored in-handle (`anchor − offset`),
/// or `None` when the anchor is a corner (no handle to grab). The Direct-Select
/// tool hit-tests and draws these.
pub fn handle_endpoints(
    points: &[(f32, f32)],
    handles: &[(f32, f32)],
    i: usize,
) -> Option<((f32, f32), (f32, f32))> {
    let p = *points.get(i)?;
    let (hx, hy) = handle_at(handles, i);
    if hx == 0.0 && hy == 0.0 {
        return None;
    }
    Some(((p.0 + hx, p.1 + hy), (p.0 - hx, p.1 - hy)))
}

/// Indices of every anchor whose point lies inside the (normalised) document-
/// space rectangle `[x, y, w, h]`. The marquee (rubber-band) anchor pick: the
/// Direct-Select tool selects all anchors caught in the dragged box. Edge-
/// touching counts as inside (matching [`rects_intersect`]).
pub fn anchors_in_rect(points: &[(f32, f32)], rect: &[f32; 4]) -> Vec<usize> {
    let x0 = rect[0].min(rect[0] + rect[2]);
    let y0 = rect[1].min(rect[1] + rect[3]);
    let x1 = rect[0].max(rect[0] + rect[2]);
    let y1 = rect[1].max(rect[1] + rect[3]);
    points
        .iter()
        .enumerate()
        .filter(|(_, &(px, py))| px >= x0 && px <= x1 && py >= y0 && py <= y1)
        .map(|(i, _)| i)
        .collect()
}

/// Turn anchor `i` into a **corner** by clearing its tangent handle (the
/// adjacent segments become straight to/from this point unless their *other*
/// endpoint still carries a handle, which keeps that side curved). Returns `true`
/// if the anchor changed (it had a handle to drop).
pub fn make_corner(handles: &mut Vec<(f32, f32)>, n: usize, i: usize) -> bool {
    if i >= n {
        return false;
    }
    if handles.len() < n {
        handles.resize(n, (0.0, 0.0));
    }
    if is_corner(handles, i) {
        return false;
    }
    handles[i] = (0.0, 0.0);
    true
}

/// Turn anchor `i` into a **smooth** point by synthesising a symmetric tangent
/// from its neighbours (parallel to the `prev → next` chord, scaled to ~1/3 of
/// it — the classic Catmull-Rom default). Returns `true` if a handle was set.
/// A no-op (returns `false`) if the anchor is already smooth or the neighbours
/// are degenerate.
pub fn make_smooth(
    points: &[(f32, f32)],
    handles: &mut Vec<(f32, f32)>,
    closed: bool,
    i: usize,
) -> bool {
    let n = points.len();
    if i >= n {
        return false;
    }
    if handles.len() < n {
        handles.resize(n, (0.0, 0.0));
    }
    if !is_corner(handles, i) {
        return false;
    }
    // Reuse the corner→smooth branch of the toggle (it only acts on corners).
    toggle_anchor_smooth(points, handles, closed, i)
}

/// Whether anchor `i` is a corner (no tangent handle) given the handle list.
pub fn is_corner(handles: &[(f32, f32)], i: usize) -> bool {
    let (hx, hy) = handle_at(handles, i);
    hx == 0.0 && hy == 0.0
}

/// Number of editable segments in a path: `n` if closed, `n - 1` if open.
pub fn segment_count(n: usize, closed: bool) -> usize {
    match (n, closed) {
        (0, _) => 0,
        (_, true) => n,
        (_, false) => n - 1,
    }
}

/// Project point `(px, py)` onto each segment of the (flattened) path and return
/// the segment index of the closest hit plus the parameter `t` along the
/// *segment's anchor pair* (0..=1), if the closest distance is within `tol`.
///
/// `t` is measured on the straight anchor-to-anchor chord, which is the right
/// parameter to feed to [`insert_anchor`] (it splits in chord parameter for a
/// line and uses de Casteljau at the same `t` for a curve — a reasonable,
/// shape-preserving split that matches user intent of "add a point here").
pub fn nearest_segment(
    points: &[(f32, f32)],
    closed: bool,
    px: f32,
    py: f32,
    tol: f32,
) -> Option<(usize, f32)> {
    let n = points.len();
    let segs = segment_count(n, closed);
    if segs == 0 {
        return None;
    }
    let mut best: Option<(usize, f32, f32)> = None; // (seg, t, dist)
    for i in 0..segs {
        let a = points[i];
        let b = points[(i + 1) % n];
        let (t, d) = project_to_segment(px, py, a, b);
        if best.is_none_or(|(_, _, bd)| d < bd) {
            best = Some((i, t, d));
        }
    }
    match best {
        Some((i, t, d)) if d <= tol => Some((i, t)),
        _ => None,
    }
}

/// Closest point on segment `a..b` to `p`: returns `(t, distance)` where `t` is
/// the clamped chord parameter in `0..=1`.
fn project_to_segment(px: f32, py: f32, a: (f32, f32), b: (f32, f32)) -> (f32, f32) {
    let (ax, ay) = a;
    let (bx, by) = b;
    let dx = bx - ax;
    let dy = by - ay;
    let len2 = dx * dx + dy * dy;
    if len2 <= 1e-9 {
        return (0.0, (px - ax).hypot(py - ay));
    }
    let t = (((px - ax) * dx + (py - ay) * dy) / len2).clamp(0.0, 1.0);
    let cx = ax + t * dx;
    let cy = ay + t * dy;
    (t, (px - cx).hypot(py - cy))
}

/// Insert a new anchor into segment `seg` at parameter `t` (0..=1), keeping the
/// path's visual shape. For a straight segment the new point is the chord
/// midpoint at `t`; for a curved segment we split the cubic with de Casteljau so
/// the two resulting curves trace the original. Mutates `points`/`handles` in
/// place and returns the index of the inserted anchor, or `None` on bad input.
pub fn insert_anchor(
    points: &mut Vec<(f32, f32)>,
    handles: &mut Vec<(f32, f32)>,
    closed: bool,
    seg: usize,
    t: f32,
) -> Option<usize> {
    let n = points.len();
    if seg >= segment_count(n, closed) {
        return None;
    }
    if handles.len() < n {
        handles.resize(n, (0.0, 0.0));
    }
    let t = t.clamp(0.0, 1.0);
    let ia = seg;
    let ib = (seg + 1) % n;
    let a = points[ia];
    let b = points[ib];
    let ha = handle_at(handles, ia);
    let hb = handle_at(handles, ib);
    let a_corner = ha.0 == 0.0 && ha.1 == 0.0;
    let b_corner = hb.0 == 0.0 && hb.1 == 0.0;

    let new_pt;
    let new_handle;
    if a_corner && b_corner {
        // Straight segment: split the chord; the new anchor is a corner.
        new_pt = (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
        new_handle = (0.0, 0.0);
    } else {
        // Cubic segment: control points are out-handle of a, in-handle of b.
        let c1 = (a.0 + ha.0, a.1 + ha.1);
        let c2 = (b.0 - hb.0, b.1 - hb.1);
        // de Casteljau at t.
        let lerp = |p: (f32, f32), q: (f32, f32)| (p.0 + (q.0 - p.0) * t, p.1 + (q.1 - p.1) * t);
        let p01 = lerp(a, c1);
        let p12 = lerp(c1, c2);
        let p23 = lerp(c2, b);
        let p012 = lerp(p01, p12);
        let p123 = lerp(p12, p23);
        let mid = lerp(p012, p123); // the new anchor, on the curve

        new_pt = mid;
        // The new anchor's out-tangent reaches toward `p123`; its in-tangent is
        // the mirror, so we store the out-offset (mid -> p123).
        new_handle = (p123.0 - mid.0, p123.1 - mid.1);

        // Re-tangent the two endpoints so the halves still trace the original
        // cubic: a's out-handle becomes a->p01, b's in-handle becomes p23->b.
        // (in-handle of b is the mirror of its stored out-offset, so we store
        // b_out = b - p23.)
        if !a_corner {
            handles[ia] = (p01.0 - a.0, p01.1 - a.1);
        }
        if !b_corner {
            handles[ib] = (b.0 - p23.0, b.1 - p23.1);
        }
    }

    let insert_at = seg + 1;
    points.insert(insert_at, new_pt);
    handles.insert(insert_at, new_handle);
    Some(insert_at)
}

/// Delete anchor `i`. Keeps `handles` aligned with `points`. Refuses to delete
/// when it would leave fewer than 2 points (the path would no longer be a
/// segment). Returns `true` if a point was removed.
pub fn delete_anchor(
    points: &mut Vec<(f32, f32)>,
    handles: &mut Vec<(f32, f32)>,
    i: usize,
) -> bool {
    if i >= points.len() || points.len() <= 2 {
        return false;
    }
    points.remove(i);
    if i < handles.len() {
        handles.remove(i);
    }
    true
}

/// Toggle anchor `i` between corner (no handle) and smooth (auto-tangent).
///
/// - If currently smooth (non-zero handle): zero it → corner. Returns `false`.
/// - If currently a corner: synthesize a symmetric tangent from the neighbour
///   anchors (the classic Catmull-Rom-style tangent: parallel to the chord
///   `prev → next`, scaled to ~1/3 of that chord) → smooth. Returns `true`.
///
/// Endpoints of an open path with only one neighbour fall back to a tangent
/// toward that single neighbour.
pub fn toggle_anchor_smooth(
    points: &[(f32, f32)],
    handles: &mut Vec<(f32, f32)>,
    closed: bool,
    i: usize,
) -> bool {
    let n = points.len();
    if i >= n {
        return false;
    }
    if handles.len() < n {
        handles.resize(n, (0.0, 0.0));
    }
    if !is_corner(handles, i) {
        handles[i] = (0.0, 0.0);
        return false;
    }
    // Corner -> smooth: derive tangent from neighbours.
    let prev = if i > 0 {
        Some(points[i - 1])
    } else if closed {
        Some(points[n - 1])
    } else {
        None
    };
    let next = if i + 1 < n {
        Some(points[i + 1])
    } else if closed {
        Some(points[0])
    } else {
        None
    };
    let p = points[i];
    let dir = match (prev, next) {
        (Some(a), Some(b)) => (b.0 - a.0, b.1 - a.1),
        (Some(a), None) => (p.0 - a.0, p.1 - a.1),
        (None, Some(b)) => (b.0 - p.0, b.1 - p.1),
        (None, None) => (0.0, 0.0),
    };
    let len = (dir.0 * dir.0 + dir.1 * dir.1).sqrt();
    if len <= 1e-6 {
        // Degenerate (coincident neighbours): leave as corner.
        return false;
    }
    // Scale to a third of the neighbour chord, a pleasant default tangent.
    let scale = len / 3.0;
    handles[i] = (dir.0 / len * scale, dir.1 / len * scale);
    true
}

/// Build a kurbo [`BezPath`] from anchor points and per-anchor out-tangent
/// handle offsets. A segment between anchors `a` and `b` is a `CurveTo` when
/// either endpoint carries a non-zero handle, otherwise a straight `LineTo`.
///
/// The out-handle of `a` is `a + handle[a]`; the in-handle of `b` is
/// `b - handle[b]` (mirror), producing smooth symmetric tangents.
pub fn bez_path(points: &[(f32, f32)], handles: &[(f32, f32)], closed: bool) -> BezPath {
    let mut els: Vec<PathEl> = Vec::with_capacity(points.len() + 2);
    let n = points.len();
    if n == 0 {
        return BezPath::from_vec(els);
    }
    let pt = |p: (f32, f32)| Point::new(p.0 as f64, p.1 as f64);
    els.push(PathEl::MoveTo(pt(points[0])));

    let seg_count = if closed { n } else { n - 1 };
    for i in 0..seg_count {
        let a = points[i];
        let b = points[(i + 1) % n];
        let ha = handle_at(handles, i);
        let hb = handle_at(handles, (i + 1) % n);
        let a_corner = ha.0 == 0.0 && ha.1 == 0.0;
        let b_corner = hb.0 == 0.0 && hb.1 == 0.0;
        if a_corner && b_corner {
            els.push(PathEl::LineTo(pt(b)));
        } else {
            let c1 = (a.0 + ha.0, a.1 + ha.1); // out-handle of a
            let c2 = (b.0 - hb.0, b.1 - hb.1); // in-handle of b (mirror)
            els.push(PathEl::CurveTo(pt(c1), pt(c2), pt(b)));
        }
    }
    if closed {
        els.push(PathEl::ClosePath);
    }
    BezPath::from_vec(els)
}

/// Flatten a (possibly bezier) path into a polyline of document-space points.
/// Used for hit-testing, polygon fills, and boolean-op input.
pub fn flatten(points: &[(f32, f32)], handles: &[(f32, f32)], closed: bool) -> Vec<(f32, f32)> {
    let any_curve = handles.iter().any(|&(hx, hy)| hx != 0.0 || hy != 0.0);
    if !any_curve {
        return points.to_vec();
    }
    let bp = bez_path(points, handles, closed);
    let mut out: Vec<(f32, f32)> = Vec::new();
    // kurbo flattens to line segments at the given tolerance (document units).
    bp.flatten(0.25, |el| match el {
        PathEl::MoveTo(p) => out.push((p.x as f32, p.y as f32)),
        PathEl::LineTo(p) => out.push((p.x as f32, p.y as f32)),
        PathEl::ClosePath => {}
        // flatten only emits MoveTo/LineTo/ClosePath.
        _ => {}
    });
    out
}

pub(super) fn point_in_rect(x: f32, y: f32, rect: &[f32; 4], tol: f32) -> bool {
    let (x0, y0) = (
        rect[0].min(rect[0] + rect[2]),
        rect[1].min(rect[1] + rect[3]),
    );
    let (x1, y1) = (
        rect[0].max(rect[0] + rect[2]),
        rect[1].max(rect[1] + rect[3]),
    );
    x >= x0 - tol && x <= x1 + tol && y >= y0 - tol && y <= y1 + tol
}

pub(super) fn dist_to_segment(px: f32, py: f32, a: (f32, f32), b: (f32, f32)) -> f32 {
    let (ax, ay) = a;
    let (bx, by) = b;
    let dx = bx - ax;
    let dy = by - ay;
    let len2 = dx * dx + dy * dy;
    if len2 <= 1e-6 {
        return (px - ax).hypot(py - ay);
    }
    let t = (((px - ax) * dx + (py - ay) * dy) / len2).clamp(0.0, 1.0);
    let cx = ax + t * dx;
    let cy = ay + t * dy;
    (px - cx).hypot(py - cy)
}

pub(super) fn point_in_polygon(px: f32, py: f32, pts: &[(f32, f32)]) -> bool {
    let mut inside = false;
    let n = pts.len();
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = pts[i];
        let (xj, yj) = pts[j];
        if (yi > py) != (yj > py) {
            let x_int = (xj - xi) * (py - yi) / (yj - yi) + xi;
            if px < x_int {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}
