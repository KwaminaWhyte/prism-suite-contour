//! Contour's vector document model.
//!
//! A document is an ordered `Vec<Shape>` (paint order: index 0 painted first,
//! last index on top). All coordinates are in *document space*; the canvas maps
//! them to screen via pan/zoom. Colors are straight sRGB RGBA in `[f32; 4]`
//! (matching egui's `Rgba`/`Color32` channel order) so they round-trip cleanly
//! through the color pickers and JSON.

use crate::transform::Affine;
use kurbo::{BezPath, PathEl, Point, Shape as KurboShape};
use prism_core::geometry::Rect as CoreRect;
use serde::{Deserialize, Serialize};

/// Default for the additive `visible` field so pre-existing `.contour` files
/// (which lack it) deserialize as visible.
fn default_true() -> bool {
    true
}

/// Line-cap style for the ends of an open stroke (and the ends of every dash).
/// Mirrors SVG's `stroke-linecap` and tiny-skia's `LineCap`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LineCap {
    /// Square end flush with the endpoint (SVG `butt`).
    #[default]
    Butt,
    /// Half-circle past the endpoint (SVG `round`).
    Round,
    /// Square end projecting half-width past the endpoint (SVG `square`).
    Square,
}

impl LineCap {
    pub const ALL: [LineCap; 3] = [LineCap::Butt, LineCap::Round, LineCap::Square];

    /// Short label for UI.
    pub fn label(self) -> &'static str {
        match self {
            LineCap::Butt => "Butt",
            LineCap::Round => "Round",
            LineCap::Square => "Square",
        }
    }

    /// SVG `stroke-linecap` keyword.
    pub fn svg(self) -> &'static str {
        match self {
            LineCap::Butt => "butt",
            LineCap::Round => "round",
            LineCap::Square => "square",
        }
    }
}

/// Line-join style for the corners where two stroke segments meet. Mirrors
/// SVG's `stroke-linejoin` and tiny-skia's `LineJoin`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LineJoin {
    /// Sharp corner, clipped to the miter limit (SVG `miter`).
    #[default]
    Miter,
    /// Rounded corner (SVG `round`).
    Round,
    /// Flattened corner (SVG `bevel`).
    Bevel,
}

impl LineJoin {
    pub const ALL: [LineJoin; 3] = [LineJoin::Miter, LineJoin::Round, LineJoin::Bevel];

    pub fn label(self) -> &'static str {
        match self {
            LineJoin::Miter => "Miter",
            LineJoin::Round => "Round",
            LineJoin::Bevel => "Bevel",
        }
    }

    pub fn svg(self) -> &'static str {
        match self {
            LineJoin::Miter => "miter",
            LineJoin::Round => "round",
            LineJoin::Bevel => "bevel",
        }
    }
}

/// Default miter limit (matches the SVG / PostScript default of 4).
fn default_miter() -> f32 {
    4.0
}

/// Non-geometry stroke attributes: caps, joins, miter limit, and a dash
/// pattern. Carried additively on every [`Shape`] via `#[serde(default)]`, so
/// pre-existing `.contour` files load as a solid butt/miter stroke.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StrokeStyle {
    #[serde(default)]
    pub cap: LineCap,
    #[serde(default)]
    pub join: LineJoin,
    #[serde(default = "default_miter")]
    pub miter_limit: f32,
    /// Dash run lengths in document units (alternating on/off). Empty = solid.
    #[serde(default)]
    pub dash: Vec<f32>,
    /// Phase offset into the dash pattern, in document units.
    #[serde(default)]
    pub dash_offset: f32,
}

impl Default for StrokeStyle {
    fn default() -> Self {
        Self {
            cap: LineCap::default(),
            join: LineJoin::default(),
            miter_limit: default_miter(),
            dash: Vec::new(),
            dash_offset: 0.0,
        }
    }
}

impl StrokeStyle {
    /// Whether this stroke draws any dashes (a non-empty pattern with at least
    /// one strictly-positive run). A pattern of all-zeros is treated as solid.
    pub fn is_dashed(&self) -> bool {
        self.dash.iter().any(|&d| d > 0.0)
    }

    /// Normalize the dash pattern for renderers that need an even-length,
    /// strictly-positive intervals list (tiny-skia, SVG): drops negatives,
    /// and if the count is odd, repeats it so on/off runs alternate cleanly
    /// (matching the SVG `stroke-dasharray` doubling rule). Returns `None` when
    /// the pattern has no positive run (i.e. should render solid).
    pub fn normalized_dash(&self) -> Option<Vec<f32>> {
        if !self.is_dashed() {
            return None;
        }
        let mut runs: Vec<f32> = self.dash.iter().map(|&d| d.max(0.0)).collect();
        if runs.len() % 2 == 1 {
            let doubled = runs.clone();
            runs.extend(doubled);
        }
        Some(runs)
    }
}

/// One drawable vector primitive.
///
/// Every variant carries an additive `visible` flag (`#[serde(default)]`) so
/// older documents keep loading. The `Path` variant additionally carries an
/// additive `handles` list describing per-anchor cubic-bezier tangents.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Shape {
    Rect {
        rect: [f32; 4],
        fill: [f32; 4],
        stroke: [f32; 4],
        stroke_w: f32,
        #[serde(default)]
        stroke_style: StrokeStyle,
        #[serde(default = "default_true")]
        visible: bool,
    },
    Ellipse {
        rect: [f32; 4],
        fill: [f32; 4],
        stroke: [f32; 4],
        stroke_w: f32,
        #[serde(default)]
        stroke_style: StrokeStyle,
        #[serde(default = "default_true")]
        visible: bool,
    },
    Line {
        p0: (f32, f32),
        p1: (f32, f32),
        stroke: [f32; 4],
        stroke_w: f32,
        #[serde(default)]
        stroke_style: StrokeStyle,
        #[serde(default = "default_true")]
        visible: bool,
    },
    Path {
        points: Vec<(f32, f32)>,
        closed: bool,
        fill: [f32; 4],
        stroke: [f32; 4],
        stroke_w: f32,
        #[serde(default)]
        stroke_style: StrokeStyle,
        /// Per-anchor *out-tangent* handle, stored as an offset (delta) from the
        /// anchor in document space. The in-tangent is the mirror (`-offset`),
        /// giving a smooth symmetric handle. `(0.0, 0.0)` means a corner anchor
        /// (the adjacent segments are straight lines).
        ///
        /// Additive: defaults to empty, in which case the path is a polyline and
        /// loads identically to the v0 model.
        #[serde(default)]
        handles: Vec<(f32, f32)>,
        #[serde(default = "default_true")]
        visible: bool,
    },
}

impl Shape {
    /// Short human label for the layer list.
    pub fn label(&self) -> &'static str {
        match self {
            Shape::Rect { .. } => "Rectangle",
            Shape::Ellipse { .. } => "Ellipse",
            Shape::Line { .. } => "Line",
            Shape::Path { .. } => "Path",
        }
    }

    /// Whether the shape is drawn / exported. Hidden shapes are skipped.
    pub fn visible(&self) -> bool {
        match self {
            Shape::Rect { visible, .. }
            | Shape::Ellipse { visible, .. }
            | Shape::Line { visible, .. }
            | Shape::Path { visible, .. } => *visible,
        }
    }

    /// Flip the visibility flag.
    pub fn toggle_visible(&mut self) {
        match self {
            Shape::Rect { visible, .. }
            | Shape::Ellipse { visible, .. }
            | Shape::Line { visible, .. }
            | Shape::Path { visible, .. } => *visible = !*visible,
        }
    }

    /// The shape's stroke attributes (caps/joins/dashes).
    pub fn stroke_style(&self) -> &StrokeStyle {
        match self {
            Shape::Rect { stroke_style, .. }
            | Shape::Ellipse { stroke_style, .. }
            | Shape::Line { stroke_style, .. }
            | Shape::Path { stroke_style, .. } => stroke_style,
        }
    }

    /// Mutable access to the shape's stroke attributes.
    pub fn stroke_style_mut(&mut self) -> &mut StrokeStyle {
        match self {
            Shape::Rect { stroke_style, .. }
            | Shape::Ellipse { stroke_style, .. }
            | Shape::Line { stroke_style, .. }
            | Shape::Path { stroke_style, .. } => stroke_style,
        }
    }

    /// Axis-aligned bounding box in document space.
    ///
    /// Returns a `prism_core::geometry::Rect` to exercise the shared suite
    /// primitive. Returns `None` for empty paths.
    pub fn bounds(&self) -> Option<CoreRect> {
        let bbox = |pts: &[(f32, f32)]| -> Option<CoreRect> {
            let mut it = pts.iter();
            let &(x0, y0) = it.next()?;
            let (mut min_x, mut min_y, mut max_x, mut max_y) = (x0, y0, x0, y0);
            for &(x, y) in it {
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
            Some(CoreRect::new(min_x, min_y, max_x - min_x, max_y - min_y))
        };
        match self {
            Shape::Rect { rect, .. } | Shape::Ellipse { rect, .. } => {
                Some(CoreRect::new(rect[0], rect[1], rect[2], rect[3]))
            }
            Shape::Line { p0, p1, .. } => bbox(&[*p0, *p1]),
            Shape::Path {
                points,
                closed,
                handles,
                ..
            } => {
                if points.is_empty() {
                    return None;
                }
                // Build the kurbo BezPath (honoring any bezier handles) and let
                // kurbo compute the tight bounding box.
                let bp = bez_path(points, handles, *closed);
                let r = bp.bounding_box();
                Some(CoreRect::new(
                    r.x0 as f32,
                    r.y0 as f32,
                    r.width() as f32,
                    r.height() as f32,
                ))
            }
        }
    }

    /// Translate every coordinate by `(dx, dy)` in document space. Handles are
    /// offsets, so they are unaffected by translation.
    pub fn translate(&mut self, dx: f32, dy: f32) {
        match self {
            Shape::Rect { rect, .. } | Shape::Ellipse { rect, .. } => {
                rect[0] += dx;
                rect[1] += dy;
            }
            Shape::Line { p0, p1, .. } => {
                p0.0 += dx;
                p0.1 += dy;
                p1.0 += dx;
                p1.1 += dy;
            }
            Shape::Path { points, .. } => {
                for p in points.iter_mut() {
                    p.0 += dx;
                    p.1 += dy;
                }
            }
        }
    }

    /// Apply an affine transform (rotate / scale / reflect / shear) to this
    /// shape, in document space.
    ///
    /// Axis-aligned shapes (`Rect`, `Ellipse`) stay their own variant only while
    /// the transform keeps their bounding box axis-aligned (pure
    /// translate/scale/flip). Under any rotation or shear they are rasterised
    /// into a [`Shape::Path`] that traces the transformed outline, exactly the
    /// way Illustrator turns a rotated rectangle into an editable path under the
    /// hood. `Line`/`Path` always transform in place (handles transform by the
    /// matrix's *linear* part, since they are offsets).
    pub fn apply_affine(&mut self, m: &Affine) {
        // A transform is "axis-aligned" if it has no rotation or shear, i.e. the
        // off-diagonal coefficients are (numerically) zero.
        let axis_aligned = m.b.abs() < 1e-6 && m.c.abs() < 1e-6;
        match self {
            Shape::Rect { rect, .. } | Shape::Ellipse { rect, .. } if axis_aligned => {
                let (x0, y0) = m.apply_point(rect[0], rect[1]);
                let (x1, y1) = m.apply_point(rect[0] + rect[2], rect[1] + rect[3]);
                // Re-normalise so width/height stay non-negative after a flip.
                rect[0] = x0.min(x1);
                rect[1] = y0.min(y1);
                rect[2] = (x1 - x0).abs();
                rect[3] = (y1 - y0).abs();
            }
            Shape::Rect { .. } | Shape::Ellipse { .. } => {
                // Rotation/shear: convert to a path tracing the outline, then
                // transform that path.
                *self = self.to_path();
                self.apply_affine(m);
            }
            Shape::Line { p0, p1, .. } => {
                *p0 = m.apply_point(p0.0, p0.1);
                *p1 = m.apply_point(p1.0, p1.1);
            }
            Shape::Path {
                points, handles, ..
            } => {
                for p in points.iter_mut() {
                    *p = m.apply_point(p.0, p.1);
                }
                for h in handles.iter_mut() {
                    *h = m.apply_vector(h.0, h.1);
                }
            }
        }
    }

    /// Convert this shape into an equivalent [`Shape::Path`], preserving paint
    /// style. `Rect` becomes a four-corner closed corner-path; `Ellipse` becomes
    /// a four-anchor closed cubic approximation; `Line` becomes a two-point open
    /// path; an existing `Path` is returned unchanged.
    pub fn to_path(&self) -> Shape {
        match self {
            Shape::Path { .. } => self.clone(),
            Shape::Rect {
                rect,
                fill,
                stroke,
                stroke_w,
                stroke_style,
                visible,
            } => {
                let pts = vec![
                    (rect[0], rect[1]),
                    (rect[0] + rect[2], rect[1]),
                    (rect[0] + rect[2], rect[1] + rect[3]),
                    (rect[0], rect[1] + rect[3]),
                ];
                let handles = vec![(0.0, 0.0); 4];
                Shape::Path {
                    points: pts,
                    closed: true,
                    fill: *fill,
                    stroke: *stroke,
                    stroke_w: *stroke_w,
                    stroke_style: stroke_style.clone(),
                    handles,
                    visible: *visible,
                }
            }
            Shape::Ellipse {
                rect,
                fill,
                stroke,
                stroke_w,
                stroke_style,
                visible,
            } => {
                // Four anchors at the extrema with the classic 0.5523 cubic
                // tangent so the path traces a smooth ellipse.
                let cx = rect[0] + rect[2] * 0.5;
                let cy = rect[1] + rect[3] * 0.5;
                let rx = rect[2] * 0.5;
                let ry = rect[3] * 0.5;
                const K: f32 = 0.552_284_8; // (4/3)·(√2−1)
                                            // Anchors clockwise from the rightmost point. Out-tangent offsets
                                            // are tangent to the ellipse, scaled by K·radius.
                let points = vec![
                    (cx + rx, cy), // right
                    (cx, cy + ry), // bottom
                    (cx - rx, cy), // left
                    (cx, cy - ry), // top
                ];
                let handles = vec![
                    (0.0, K * ry),  // right anchor: tangent down
                    (-K * rx, 0.0), // bottom anchor: tangent left
                    (0.0, -K * ry), // left anchor: tangent up
                    (K * rx, 0.0),  // top anchor: tangent right
                ];
                Shape::Path {
                    points,
                    closed: true,
                    fill: *fill,
                    stroke: *stroke,
                    stroke_w: *stroke_w,
                    stroke_style: stroke_style.clone(),
                    handles,
                    visible: *visible,
                }
            }
            Shape::Line {
                p0,
                p1,
                stroke,
                stroke_w,
                stroke_style,
                visible,
            } => Shape::Path {
                points: vec![*p0, *p1],
                closed: false,
                fill: [0.0, 0.0, 0.0, 0.0],
                stroke: *stroke,
                stroke_w: *stroke_w,
                stroke_style: stroke_style.clone(),
                handles: vec![(0.0, 0.0); 2],
                visible: *visible,
            },
        }
    }

    /// Insert an anchor into this path at segment `seg`, parameter `t`,
    /// preserving shape. No-op (returns `None`) on non-`Path` shapes.
    pub fn insert_anchor(&mut self, seg: usize, t: f32) -> Option<usize> {
        if let Shape::Path {
            points,
            closed,
            handles,
            ..
        } = self
        {
            insert_anchor(points, handles, *closed, seg, t)
        } else {
            None
        }
    }

    /// Delete anchor `i` from this path (keeps ≥2 points). No-op on non-`Path`.
    pub fn delete_anchor(&mut self, i: usize) -> bool {
        if let Shape::Path {
            points, handles, ..
        } = self
        {
            delete_anchor(points, handles, i)
        } else {
            false
        }
    }

    /// Toggle anchor `i` smooth↔corner on this path. Returns the new smooth
    /// state (`true` = now smooth). No-op (returns `false`) on non-`Path`.
    pub fn toggle_anchor_smooth(&mut self, i: usize) -> bool {
        if let Shape::Path {
            points,
            closed,
            handles,
            ..
        } = self
        {
            toggle_anchor_smooth(points, handles, *closed, i)
        } else {
            false
        }
    }

    /// Hit-test a document-space point. Tolerance is in document units (used to
    /// give lines/open paths a clickable thickness).
    pub fn hit(&self, x: f32, y: f32, tol: f32) -> bool {
        match self {
            Shape::Rect { rect, .. } => point_in_rect(x, y, rect, tol),
            Shape::Ellipse { rect, .. } => {
                let cx = rect[0] + rect[2] * 0.5;
                let cy = rect[1] + rect[3] * 0.5;
                let rx = (rect[2] * 0.5).max(1e-3);
                let ry = (rect[3] * 0.5).max(1e-3);
                let nx = (x - cx) / (rx + tol);
                let ny = (y - cy) / (ry + tol);
                nx * nx + ny * ny <= 1.0
            }
            Shape::Line { p0, p1, .. } => dist_to_segment(x, y, *p0, *p1) <= tol.max(2.0),
            Shape::Path {
                points,
                closed,
                handles,
                ..
            } => {
                // Hit-test against the flattened polyline so curves are clickable.
                let flat = flatten(points, handles, *closed);
                if *closed && flat.len() >= 3 && point_in_polygon(x, y, &flat) {
                    return true;
                }
                let n = flat.len();
                if n < 2 {
                    return n == 1 && (x - flat[0].0).hypot(y - flat[0].1) <= tol.max(2.0);
                }
                let last = if *closed { n } else { n - 1 };
                for i in 0..last {
                    let a = flat[i];
                    let b = flat[(i + 1) % n];
                    if dist_to_segment(x, y, a, b) <= tol.max(2.0) {
                        return true;
                    }
                }
                false
            }
        }
    }
}

/// A shape's index in the document paired with its bounding box. Used by the
/// align/distribute layer, which needs to map per-box translation deltas back to
/// the originating shape.
#[derive(Clone, Copy, Debug)]
pub struct ShapeBounds {
    pub index: usize,
    pub rect: CoreRect,
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

/// Whether anchor `i` is a corner (no tangent handle) given the handle list.
pub fn is_corner(handles: &[(f32, f32)], i: usize) -> bool {
    let (hx, hy) = handle_at(handles, i);
    hx == 0.0 && hy == 0.0
}

// --- Direct-select path editing (pure logic) --------------------------------
//
// These operate on the `(points, handles, closed)` triple that backs a
// `Shape::Path`. They keep `handles` the same length as `points` and are written
// to be unit-testable without any UI. The app layer calls them via the
// `Shape::Path` mutation helpers below.

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

fn point_in_rect(x: f32, y: f32, rect: &[f32; 4], tol: f32) -> bool {
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

fn dist_to_segment(px: f32, py: f32, a: (f32, f32), b: (f32, f32)) -> f32 {
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

fn point_in_polygon(px: f32, py: f32, pts: &[(f32, f32)]) -> bool {
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

/// A ruler guide: an infinite straight line at a fixed document coordinate,
/// either vertical (constant `x`) or horizontal (constant `y`). Dragged out of
/// the rulers and used as a snap target. Stored on the [`Document`] so guides
/// persist in `.contour` files.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Guide {
    /// A vertical line at this `x` (in document units).
    Vertical(f32),
    /// A horizontal line at this `y` (in document units).
    Horizontal(f32),
}

/// The whole vector document: an ordered list of shapes plus any ruler guides.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Document {
    pub shapes: Vec<Shape>,
    /// User-placed ruler guides. Additive (`#[serde(default)]`) so pre-existing
    /// `.contour` files — which have no `guides` key — load with none.
    #[serde(default)]
    pub guides: Vec<Guide>,
}

impl Document {
    pub fn new() -> Self {
        Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Old `.contour` JSON (pre-`visible`/`handles`) must still deserialize,
    /// defaulting `visible = true` and `handles = []`.
    #[test]
    fn loads_legacy_document() {
        let json = r#"{"shapes":[
            {"Rect":{"rect":[0,0,10,10],"fill":[1,0,0,1],"stroke":[0,0,0,1],"stroke_w":2}},
            {"Path":{"points":[[0,0],[10,0],[10,10]],"closed":true,"fill":[0,1,0,1],"stroke":[0,0,0,1],"stroke_w":1}}
        ]}"#;
        let doc: Document = serde_json::from_str(json).unwrap();
        assert_eq!(doc.shapes.len(), 2);
        assert!(doc.shapes[0].visible());
        assert!(doc.shapes[1].visible());
        // A pre-guides document loads with no guides.
        assert!(doc.guides.is_empty());
        if let Shape::Path { handles, .. } = &doc.shapes[1] {
            assert!(handles.is_empty());
        } else {
            panic!("expected Path");
        }
    }

    /// Guides round-trip through serde and load back as the same variant.
    #[test]
    fn guides_round_trip() {
        let mut doc = Document::new();
        doc.guides.push(Guide::Vertical(100.0));
        doc.guides.push(Guide::Horizontal(42.5));
        let json = serde_json::to_string(&doc).unwrap();
        let back: Document = serde_json::from_str(&json).unwrap();
        assert_eq!(
            back.guides,
            vec![Guide::Vertical(100.0), Guide::Horizontal(42.5)]
        );
    }

    #[test]
    fn rects_intersect_basic_cases() {
        let a = [0.0, 0.0, 10.0, 10.0];
        // Overlapping.
        assert!(rects_intersect(&a, &[5.0, 5.0, 10.0, 10.0]));
        // Fully inside.
        assert!(rects_intersect(&a, &[2.0, 2.0, 2.0, 2.0]));
        // Contains a (b bigger).
        assert!(rects_intersect(&a, &[-5.0, -5.0, 100.0, 100.0]));
        // Edge-touching counts.
        assert!(rects_intersect(&a, &[10.0, 0.0, 5.0, 5.0]));
        // Disjoint on x.
        assert!(!rects_intersect(&a, &[20.0, 0.0, 5.0, 5.0]));
        // Disjoint on y.
        assert!(!rects_intersect(&a, &[0.0, 20.0, 5.0, 5.0]));
    }

    /// A path with no handles flattens to its raw points (polyline).
    #[test]
    fn flatten_polyline_is_identity() {
        let pts = vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)];
        let out = flatten(&pts, &[], false);
        assert_eq!(out, pts);
    }

    /// A path with a non-zero handle flattens into more segments (a curve).
    #[test]
    fn flatten_curve_subdivides() {
        let pts = vec![(0.0, 0.0), (100.0, 0.0)];
        let handles = vec![(0.0, 50.0), (0.0, 50.0)];
        let out = flatten(&pts, &handles, false);
        assert!(out.len() > 2, "curve should subdivide, got {}", out.len());
    }

    // --- Direct-select path editing ----------------------------------------

    #[test]
    fn segment_count_open_vs_closed() {
        assert_eq!(segment_count(0, false), 0);
        assert_eq!(segment_count(0, true), 0);
        assert_eq!(segment_count(3, false), 2);
        assert_eq!(segment_count(3, true), 3);
    }

    #[test]
    fn nearest_segment_picks_closest() {
        // Square-ish open path; click near the middle of the first segment.
        let pts = vec![(0.0, 0.0), (100.0, 0.0), (100.0, 100.0)];
        let (seg, t) = nearest_segment(&pts, false, 50.0, 1.0, 5.0).expect("hit");
        assert_eq!(seg, 0);
        assert!((t - 0.5).abs() < 1e-3, "t={t}");
    }

    #[test]
    fn nearest_segment_misses_when_far() {
        let pts = vec![(0.0, 0.0), (100.0, 0.0)];
        assert!(nearest_segment(&pts, false, 50.0, 50.0, 5.0).is_none());
    }

    #[test]
    fn nearest_segment_closed_uses_wrap_segment() {
        // Triangle; click near the closing edge (last anchor back to first).
        let pts = vec![(0.0, 0.0), (100.0, 0.0), (50.0, 100.0)];
        // Midpoint of closing segment (idx 2): (25, 50).
        let (seg, _t) = nearest_segment(&pts, true, 25.0, 50.0, 5.0).expect("hit");
        assert_eq!(seg, 2);
    }

    #[test]
    fn insert_anchor_splits_straight_segment_at_midpoint() {
        let mut pts = vec![(0.0, 0.0), (100.0, 0.0)];
        let mut handles = vec![(0.0, 0.0), (0.0, 0.0)];
        let idx = insert_anchor(&mut pts, &mut handles, false, 0, 0.5).expect("inserted");
        assert_eq!(idx, 1);
        assert_eq!(pts.len(), 3);
        assert_eq!(handles.len(), 3);
        assert_eq!(pts[1], (50.0, 0.0));
        // New anchor on a straight segment is a corner.
        assert!(is_corner(&handles, 1));
    }

    #[test]
    fn insert_anchor_on_curve_preserves_shape() {
        // A cubic segment; inserting at t splits it via de Casteljau, so the new
        // anchor must land exactly on the original cubic evaluated at t, and the
        // endpoints must be untouched.
        let a = (0.0, 0.0);
        let b = (100.0, 0.0);
        let pts = vec![a, b];
        let handles = vec![(30.0, 60.0), (30.0, -60.0)]; // both smooth

        // Original cubic control points (mirror in-handle of b).
        let c1 = (a.0 + handles[0].0, a.1 + handles[0].1);
        let c2 = (b.0 - handles[1].0, b.1 - handles[1].1);
        let t = 0.5_f32;
        let cubic = |t: f32| {
            let mt = 1.0 - t;
            let x = mt * mt * mt * a.0
                + 3.0 * mt * mt * t * c1.0
                + 3.0 * mt * t * t * c2.0
                + t * t * t * b.0;
            let y = mt * mt * mt * a.1
                + 3.0 * mt * mt * t * c1.1
                + 3.0 * mt * t * t * c2.1
                + t * t * t * b.1;
            (x, y)
        };
        let expected = cubic(t);

        let mut pts2 = pts.clone();
        let mut handles2 = handles.clone();
        let idx = insert_anchor(&mut pts2, &mut handles2, false, 0, t).expect("inserted");
        assert_eq!(idx, 1);
        assert_eq!(pts2.len(), 3);

        // Endpoints unchanged.
        assert_eq!(pts2[0], a);
        assert_eq!(pts2[2], b);
        // New anchor lies exactly on the original cubic at t.
        let mid = pts2[1];
        assert!(
            (mid.0 - expected.0).abs() < 1e-3 && (mid.1 - expected.1).abs() < 1e-3,
            "inserted {mid:?} != on-curve {expected:?}"
        );

        // And the split halves still trace the original curve: sample several
        // points on the new (two-segment) path against the original cubic.
        let after = flatten(&pts2, &handles2, false);
        for &(x, y) in &after {
            // nearest distance from this point to the original cubic (dense sample)
            let mut min_d = f32::INFINITY;
            for s in 0..=200 {
                let cp = cubic(s as f32 / 200.0);
                min_d = min_d.min((x - cp.0).hypot(y - cp.1));
            }
            assert!(
                min_d < 0.5,
                "split point ({x},{y}) off original curve by {min_d}"
            );
        }
    }

    #[test]
    fn delete_anchor_keeps_min_two_points() {
        let mut pts = vec![(0.0, 0.0), (10.0, 0.0), (20.0, 0.0)];
        let mut handles = vec![(0.0, 0.0); 3];
        assert!(delete_anchor(&mut pts, &mut handles, 1));
        assert_eq!(pts, vec![(0.0, 0.0), (20.0, 0.0)]);
        assert_eq!(handles.len(), 2);
        // Now at 2 points: refuse to delete further.
        assert!(!delete_anchor(&mut pts, &mut handles, 0));
        assert_eq!(pts.len(), 2);
    }

    #[test]
    fn toggle_anchor_corner_to_smooth_and_back() {
        let pts = vec![(0.0, 0.0), (100.0, 0.0), (200.0, 0.0)];
        let mut handles = vec![(0.0, 0.0); 3];
        // Middle anchor, neighbours straddle horizontally -> horizontal tangent.
        let now_smooth = toggle_anchor_smooth(&pts, &mut handles, false, 1);
        assert!(now_smooth);
        assert!(!is_corner(&handles, 1));
        // Tangent should be ~horizontal (dir prev->next is +x).
        let (hx, hy) = handles[1];
        assert!(hx > 0.0 && hy.abs() < 1e-3, "handle=({hx},{hy})");
        // Toggle again -> corner.
        let now_smooth = toggle_anchor_smooth(&pts, &mut handles, false, 1);
        assert!(!now_smooth);
        assert!(is_corner(&handles, 1));
    }

    #[test]
    fn toggle_anchor_endpoint_uses_single_neighbour() {
        let pts = vec![(0.0, 0.0), (100.0, 0.0)];
        let mut handles = vec![(0.0, 0.0); 2];
        // First anchor of an open path: tangent toward the only neighbour.
        let now_smooth = toggle_anchor_smooth(&pts, &mut handles, false, 0);
        assert!(now_smooth);
        let (hx, hy) = handles[0];
        assert!(hx > 0.0 && hy.abs() < 1e-3);
    }

    // --- Stroke style ------------------------------------------------------

    #[test]
    fn stroke_style_default_is_solid_butt_miter() {
        let s = StrokeStyle::default();
        assert_eq!(s.cap, LineCap::Butt);
        assert_eq!(s.join, LineJoin::Miter);
        assert_eq!(s.miter_limit, 4.0);
        assert!(!s.is_dashed());
        assert!(s.normalized_dash().is_none());
    }

    #[test]
    fn is_dashed_ignores_all_zero_pattern() {
        let solid = StrokeStyle {
            dash: vec![0.0, 0.0],
            ..Default::default()
        };
        assert!(!solid.is_dashed());
        let dashed = StrokeStyle {
            dash: vec![6.0, 3.0],
            ..Default::default()
        };
        assert!(dashed.is_dashed());
    }

    #[test]
    fn normalized_dash_doubles_odd_pattern() {
        // Odd-length pattern must be repeated so on/off runs alternate evenly
        // (the SVG stroke-dasharray rule).
        let s = StrokeStyle {
            dash: vec![5.0],
            ..Default::default()
        };
        let n = s.normalized_dash().expect("dashed");
        assert_eq!(n, vec![5.0, 5.0]);

        let s2 = StrokeStyle {
            dash: vec![6.0, 2.0, 1.0],
            ..Default::default()
        };
        let n2 = s2.normalized_dash().expect("dashed");
        assert_eq!(n2, vec![6.0, 2.0, 1.0, 6.0, 2.0, 1.0]);
    }

    #[test]
    fn normalized_dash_clamps_negatives() {
        let s = StrokeStyle {
            dash: vec![6.0, -2.0],
            ..Default::default()
        };
        let n = s.normalized_dash().expect("has a positive run");
        assert_eq!(n, vec![6.0, 0.0]);
    }

    // --- Affine transforms -------------------------------------------------

    #[test]
    fn axis_aligned_scale_keeps_rect_a_rect() {
        let mut s = Shape::Rect {
            rect: [10.0, 20.0, 40.0, 30.0],
            fill: [0.0; 4],
            stroke: [0.0; 4],
            stroke_w: 1.0,
            stroke_style: StrokeStyle::default(),
            visible: true,
        };
        // Scale ×2 about the origin.
        s.apply_affine(&Affine::scale(2.0, 2.0));
        match s {
            Shape::Rect { rect, .. } => {
                assert_eq!(rect, [20.0, 40.0, 80.0, 60.0]);
            }
            _ => panic!("axis-aligned scale should keep a Rect a Rect"),
        }
    }

    #[test]
    fn flip_keeps_rect_normalized() {
        let mut s = Shape::Rect {
            rect: [10.0, 0.0, 40.0, 20.0],
            fill: [0.0; 4],
            stroke: [0.0; 4],
            stroke_w: 1.0,
            stroke_style: StrokeStyle::default(),
            visible: true,
        };
        // Horizontal flip about x = 30 (the rect's centre): bounds unchanged,
        // width/height stay positive.
        s.apply_affine(&Affine::flip_h_about(30.0));
        match s {
            Shape::Rect { rect, .. } => {
                assert!((rect[0] - 10.0).abs() < 1e-3);
                assert!((rect[2] - 40.0).abs() < 1e-3);
                assert!(rect[2] > 0.0 && rect[3] > 0.0);
            }
            _ => panic!("flip should keep a Rect a Rect"),
        }
    }

    #[test]
    fn rotation_converts_rect_to_path() {
        let mut s = Shape::Rect {
            rect: [0.0, 0.0, 10.0, 10.0],
            fill: [0.0; 4],
            stroke: [0.0; 4],
            stroke_w: 1.0,
            stroke_style: StrokeStyle::default(),
            visible: true,
        };
        s.apply_affine(&Affine::rotate_about(0.5, 5.0, 5.0));
        assert!(
            matches!(s, Shape::Path { .. }),
            "rotation must rasterise to a Path"
        );
        if let Shape::Path { points, closed, .. } = &s {
            assert_eq!(points.len(), 4);
            assert!(*closed);
        }
    }

    #[test]
    fn rotation_preserves_rect_center() {
        let mut s = Shape::Rect {
            rect: [0.0, 0.0, 100.0, 40.0],
            fill: [0.0; 4],
            stroke: [0.0; 4],
            stroke_w: 1.0,
            stroke_style: StrokeStyle::default(),
            visible: true,
        };
        let before = s.bounds().unwrap();
        let (cx0, cy0) = (before.x + before.w * 0.5, before.y + before.h * 0.5);
        // Rotate 90° about the rect centre; the centre must be fixed.
        s.apply_affine(&Affine::rotate_about(std::f32::consts::FRAC_PI_2, cx0, cy0));
        let after = s.bounds().unwrap();
        let (cx1, cy1) = (after.x + after.w * 0.5, after.y + after.h * 0.5);
        assert!((cx0 - cx1).abs() < 0.5 && (cy0 - cy1).abs() < 0.5);
        // A 90° turn swaps the bbox extents.
        assert!((after.w - before.h).abs() < 0.5);
        assert!((after.h - before.w).abs() < 0.5);
    }

    #[test]
    fn ellipse_to_path_round_trips_bounds() {
        let s = Shape::Ellipse {
            rect: [0.0, 0.0, 80.0, 40.0],
            fill: [0.0; 4],
            stroke: [0.0; 4],
            stroke_w: 1.0,
            stroke_style: StrokeStyle::default(),
            visible: true,
        };
        let p = s.to_path();
        let pb = p.bounds().unwrap();
        // The cubic ellipse path should hug the original ellipse box closely.
        assert!((pb.x - 0.0).abs() < 0.5);
        assert!((pb.y - 0.0).abs() < 0.5);
        assert!((pb.w - 80.0).abs() < 0.5);
        assert!((pb.h - 40.0).abs() < 0.5);
    }

    #[test]
    fn path_handles_transform_by_linear_part() {
        // A path with a curve handle; under a translate the handle (an offset)
        // must NOT move, but the anchors must.
        let mut s = Shape::Path {
            points: vec![(0.0, 0.0), (10.0, 0.0)],
            closed: false,
            fill: [0.0; 4],
            stroke: [0.0; 4],
            stroke_w: 1.0,
            stroke_style: StrokeStyle::default(),
            handles: vec![(3.0, 4.0), (0.0, 0.0)],
            visible: true,
        };
        s.apply_affine(&Affine::translate(100.0, 50.0));
        if let Shape::Path {
            points, handles, ..
        } = &s
        {
            assert_eq!(points[0], (100.0, 50.0));
            assert_eq!(handles[0], (3.0, 4.0)); // offset unchanged by translation
        } else {
            panic!("still a path");
        }
    }

    /// `.contour` files written before stroke styles existed must load with a
    /// default (solid, butt, miter) stroke style on every shape.
    #[test]
    fn loads_pre_stroke_style_document() {
        let json = r#"{"shapes":[
            {"Line":{"p0":[0,0],"p1":[10,0],"stroke":[0,0,0,1],"stroke_w":2}}
        ]}"#;
        let doc: Document = serde_json::from_str(json).unwrap();
        assert_eq!(doc.shapes.len(), 1);
        let st = doc.shapes[0].stroke_style();
        assert_eq!(st, &StrokeStyle::default());
    }
}
